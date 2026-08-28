//! `yocompat`, the differential tester.
//!
//! It sends the same command to a real Redis and to yodb, reads both replies,
//! and complains when they are not the same bytes. That is the whole idea. A
//! compatibility test written from the documentation tests the documentation,
//! and Redis's real behaviour and Redis's documented behaviour part company
//! often enough that the only thing worth comparing against is a running
//! server.
//!
//! Redis's own Tcl suite is the other half of this repository and it is the
//! stronger of the two where it applies. This half covers what that one cannot:
//! the exact reply type, the exact error word, and RESP3 shapes, on commands
//! their suite happens not to reach.

mod corpus;
mod resp;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use corpus::{Corpus, Step};
use resp::{Conn, Reply};

const USAGE: &str = "\
yocompat, the differential tester

usage:
  yocompat [options] [corpus ...]

options:
  --yodb PATH      our server, ../yo/target/release/yodb by default
  --redis PATH     a real redis-server, found on PATH by default
  --resp3          say HELLO 3 first, so the RESP3 shapes get compared
  --quiet          only print divergences and the summary
  --corpus DIR     where the corpus files are, corpus/ by default

exit codes:
  0  every reply matched, or every mismatch was already registered
  1  something diverged that is not in the register
  2  the arguments did not make sense, or a server would not start
";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("yocompat: {e}");
            std::process::ExitCode::from(2)
        }
    }
}

struct Opts {
    yodb: PathBuf,
    redis: PathBuf,
    resp3: bool,
    quiet: bool,
    dir: PathBuf,
    files: Vec<PathBuf>,
}

/// One command where the two servers disagreed.
struct Divergence {
    file: String,
    line: usize,
    command: String,
    kind: &'static str,
    ours: String,
    theirs: String,
}

/// Say what sort of disagreement this is.
///
/// Not every mismatch costs the same. Accepting a command Redis refuses is a
/// correctness bug and a client that relies on the refusal will corrupt data
/// against us. Refusing it with a differently worded message is a bug too, but a
/// smaller one, and a client that branches on the first word will not even
/// notice. Sorting them apart in the report is the difference between a list you
/// can work through and a list you scroll past.
fn classify(ours: &Result<Reply, String>, theirs: &Result<Reply, String>) -> &'static str {
    let (Ok(ours), Ok(theirs)) = (ours, theirs) else {
        return "one of them did not answer";
    };
    match (ours.error_word(), theirs.error_word()) {
        (Some(a), Some(b)) if a == b => "same error, different wording",
        (Some(_), Some(_)) => "both refused it, with different errors",
        (None, Some(_)) => "we accepted what redis refuses",
        (Some(_), None) => "we refused what redis accepts",
        (None, None) => "different reply",
    }
}

fn run() -> Result<std::process::ExitCode, String> {
    let Some(opts) = parse()? else {
        return Ok(std::process::ExitCode::SUCCESS);
    };

    let files = if opts.files.is_empty() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&opts.dir)
            .map_err(|e| format!("{}: {e}", opts.dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "txt"))
            .collect();
        v.sort();
        v
    } else {
        opts.files.clone()
    };
    if files.is_empty() {
        return Err(format!("no corpus files in {}", opts.dir.display()));
    }

    let register = corpus::Register::load(opts.dir.parent().unwrap_or(Path::new(".")))?;

    let mut diverged: Vec<Divergence> = Vec::new();
    let mut registered = 0usize;
    let mut compared = 0usize;

    for file in &files {
        let corpus = Corpus::load(file)?;
        let name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !opts.quiet {
            eprintln!("== {name}: {} commands", corpus.steps.len());
        }

        // Both servers get a fresh process per corpus file. We have no
        // FLUSHALL yet, and a corpus that starts from whatever the last one
        // left behind is a corpus that passes or fails depending on the order
        // the files were read in.
        let mut ours = Server::start(&opts.yodb, &["serve", "--port", "7521"], 7521)?;
        let mut theirs = Server::start(
            &opts.redis,
            &["--port", "7522", "--save", "", "--appendonly", "no"],
            7522,
        )?;

        let mut a = Conn::connect(7521).map_err(|e| format!("yodb: {e}"))?;
        let mut b = Conn::connect(7522).map_err(|e| format!("redis: {e}"))?;

        if opts.resp3 {
            let hello = vec!["HELLO".to_string(), "3".to_string()];
            let _ = talk(&mut a, &hello);
            let _ = talk(&mut b, &hello);
        }

        for step in &corpus.steps {
            let Step { line, args } = step;
            let mine = talk(&mut a, args);
            let theirs_reply = talk(&mut b, args);
            compared += 1;

            let kind = classify(&mine, &theirs_reply);
            let mine = mine
                .map(|r| r.render())
                .unwrap_or_else(|e| format!("<{e}>"));
            let theirs_reply = theirs_reply
                .map(|r| r.render())
                .unwrap_or_else(|e| format!("<{e}>"));

            if mine == theirs_reply {
                continue;
            }
            let command = args.join(" ");
            if register.excuses(&name, &command) {
                registered += 1;
                continue;
            }
            diverged.push(Divergence {
                file: name.clone(),
                line: *line,
                command,
                kind,
                ours: mine,
                theirs: theirs_reply,
            });
        }

        ours.stop();
        theirs.stop();
    }

    println!("\n{compared} commands compared against redis");
    if registered > 0 {
        println!("{registered} known divergences, all of them in divergences.toml");
    }

    if diverged.is_empty() {
        println!("nothing else diverged");
        return Ok(std::process::ExitCode::SUCCESS);
    }

    println!("\n{} unregistered divergences:\n", diverged.len());
    // Worst first. A wall of wording differences at the top of the report is how
    // the one command we get outright wrong gets missed.
    let order = [
        "we accepted what redis refuses",
        "different reply",
        "we refused what redis accepts",
        "both refused it, with different errors",
        "one of them did not answer",
        "same error, different wording",
    ];
    for kind in order {
        let group: Vec<&Divergence> = diverged.iter().filter(|d| d.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        println!("-- {kind} ({})", group.len());
        for d in group {
            println!("{}:{}  {}", d.file, d.line, d.command);
            println!("  redis: {}", d.theirs);
            println!("  yo:    {}", d.ours);
        }
        println!();
    }
    println!(
        "\nEach of these is either a bug to fix or a line to add to divergences.toml with a reason."
    );
    Ok(std::process::ExitCode::FAILURE)
}

fn talk(conn: &mut Conn, args: &[String]) -> Result<Reply, String> {
    conn.send(args).map_err(|e| e.to_string())?;
    conn.read().map_err(|e| e.to_string())
}

/// A server under test, started and stopped by us.
struct Server {
    child: Child,
}

impl Server {
    fn start(bin: &Path, args: &[&str], port: u16) -> Result<Server, String> {
        let child = Command::new(bin)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{}: {e}", bin.display()))?;

        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(mut c) = Conn::connect(port)
                && c.send(&["PING".to_string()]).is_ok()
                && c.read().is_ok()
            {
                return Ok(Server { child });
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let mut s = Server { child };
        s.stop();
        Err(format!("{} never answered on {port}", bin.display()))
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn parse() -> Result<Option<Opts>, String> {
    let mut o = Opts {
        yodb: PathBuf::from("../yo/target/release/yodb"),
        redis: PathBuf::from("redis-server"),
        resp3: false,
        quiet: false,
        dir: PathBuf::from("corpus"),
        files: Vec::new(),
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut at = 0;
    while at < args.len() {
        let arg = args[at].as_str();
        at += 1;
        let mut value = || -> Result<String, String> {
            let v = args.get(at).ok_or(format!("{arg} needs a value"))?.clone();
            at += 1;
            Ok(v)
        };
        match arg {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--resp3" => o.resp3 = true,
            "--quiet" => o.quiet = true,
            "--yodb" => o.yodb = PathBuf::from(value()?),
            "--redis" => o.redis = PathBuf::from(value()?),
            "--corpus" => o.dir = PathBuf::from(value()?),
            other if other.starts_with('-') => return Err(format!("no such option: {other}")),
            other => o.files.push(PathBuf::from(other)),
        }
    }
    Ok(Some(o))
}
