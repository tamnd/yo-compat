//! A RESP client that is deliberately naive.
//!
//! It has to read whatever both servers send without helping either of them.
//! A client with a type system that turns two different wire encodings into the
//! same Rust value would hide exactly the bugs this repository exists to find,
//! so a reply is kept as its shape and its bytes and compared that way.

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A reply, kept in the shape it arrived in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// `+OK`
    Simple(Vec<u8>),
    /// `-ERR something`
    Error(Vec<u8>),
    /// `:42`
    Int(Vec<u8>),
    /// `$3\r\nfoo`, and `=` for a verbatim string, kept apart because they are
    /// different types on the wire even when they carry the same bytes.
    Bulk(Vec<u8>),
    /// `=14\r\ntxt:...`
    Verbatim(Vec<u8>),
    /// `!21\r\n...`, a RESP3 bulk error.
    BulkError(Vec<u8>),
    /// `,3.14`
    Double(Vec<u8>),
    /// `#t`
    Bool(Vec<u8>),
    /// `(1234567890`
    BigNumber(Vec<u8>),
    /// `_`, and `$-1` and `*-1`, which are the RESP2 spellings of the same idea
    /// and are kept apart from it.
    Null,
    /// `$-1`
    NullBulk,
    /// `*-1`
    NullArray,
    /// `*2\r\n...`
    Array(Vec<Reply>),
    /// `~2\r\n...`
    Set(Vec<Reply>),
    /// `>2\r\n...`
    Push(Vec<Reply>),
    /// `%1\r\n...`, kept as the flat sequence it arrives in.
    Map(Vec<Reply>),
}

impl Reply {
    /// A one line form for a report and for comparing.
    ///
    /// This is the only place two replies are turned into text, so two replies
    /// that print the same really are the same as far as anything downstream is
    /// concerned.
    pub fn render(&self) -> String {
        match self {
            Reply::Simple(b) => format!("+{}", show(b)),
            Reply::Error(b) => format!("-{}", show(b)),
            Reply::Int(b) => format!(":{}", show(b)),
            Reply::Bulk(b) => format!("${}", show(b)),
            Reply::Verbatim(b) => format!("={}", show(b)),
            Reply::BulkError(b) => format!("!{}", show(b)),
            Reply::Double(b) => format!(",{}", show(b)),
            Reply::Bool(b) => format!("#{}", show(b)),
            Reply::BigNumber(b) => format!("({}", show(b)),
            Reply::Null => "_".to_string(),
            Reply::NullBulk => "$-1".to_string(),
            Reply::NullArray => "*-1".to_string(),
            Reply::Array(v) => format!("*[{}]", join(v)),
            Reply::Set(v) => format!("~[{}]", join(v)),
            Reply::Push(v) => format!(">[{}]", join(v)),
            Reply::Map(v) => format!("%[{}]", join(v)),
        }
    }

    /// The error message, if it is one. Used to tell "we said no differently"
    /// from "we said yes when they said no", which are not the same bug.
    pub fn error_word(&self) -> Option<String> {
        let bytes = match self {
            Reply::Error(b) | Reply::BulkError(b) => b,
            _ => return None,
        };
        let text = String::from_utf8_lossy(bytes);
        Some(text.split_whitespace().next().unwrap_or("").to_string())
    }
}

fn join(v: &[Reply]) -> String {
    v.iter().map(Reply::render).collect::<Vec<_>>().join(", ")
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// One connection to one server.
pub struct Conn {
    out: TcpStream,
    input: BufReader<TcpStream>,
}

impl Conn {
    /// Open it, with timeouts, so a server that stops answering fails the run
    /// rather than hanging it.
    pub fn connect(port: u16) -> io::Result<Conn> {
        let stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let input = BufReader::new(stream.try_clone()?);
        Ok(Conn { out: stream, input })
    }

    /// Send one command as a RESP array of bulk strings.
    ///
    /// Always an array, never inline, because inline commands take a different
    /// path through both servers and this is not the place to be testing that.
    pub fn send(&mut self, args: &[String]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
        for a in args {
            buf.extend_from_slice(format!("${}\r\n", a.len()).as_bytes());
            buf.extend_from_slice(a.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        self.out.write_all(&buf)?;
        self.out.flush()
    }

    /// Read exactly one reply.
    pub fn read(&mut self) -> io::Result<Reply> {
        let line = self.line()?;
        let (tag, rest) = line
            .split_first()
            .ok_or_else(|| io::Error::other("empty reply"))?;
        let rest = rest.to_vec();
        match tag {
            b'+' => Ok(Reply::Simple(rest)),
            b'-' => Ok(Reply::Error(rest)),
            b':' => Ok(Reply::Int(rest)),
            b',' => Ok(Reply::Double(rest)),
            b'#' => Ok(Reply::Bool(rest)),
            b'(' => Ok(Reply::BigNumber(rest)),
            b'_' => Ok(Reply::Null),
            b'$' | b'=' | b'!' => {
                let n = number(&rest)?;
                if n < 0 {
                    return Ok(Reply::NullBulk);
                }
                let body = self.exact(n as usize)?;
                Ok(match tag {
                    b'$' => Reply::Bulk(body),
                    b'=' => Reply::Verbatim(body),
                    _ => Reply::BulkError(body),
                })
            }
            b'*' | b'~' | b'>' | b'%' => {
                let n = number(&rest)?;
                if n < 0 {
                    return Ok(Reply::NullArray);
                }
                let items = if *tag == b'%' {
                    n as usize * 2
                } else {
                    n as usize
                };
                let mut v = Vec::with_capacity(items);
                for _ in 0..items {
                    v.push(self.read()?);
                }
                Ok(match tag {
                    b'*' => Reply::Array(v),
                    b'~' => Reply::Set(v),
                    b'>' => Reply::Push(v),
                    _ => Reply::Map(v),
                })
            }
            other => Err(io::Error::other(format!(
                "not a reply type: {:?}",
                *other as char
            ))),
        }
    }

    fn line(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        let n = self.input.read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Err(io::Error::other("the server closed the connection"));
        }
        while buf.last() == Some(&b'\n') || buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Ok(buf)
    }

    fn exact(&mut self, n: usize) -> io::Result<Vec<u8>> {
        use std::io::Read as _;
        let mut body = vec![0u8; n + 2];
        self.input.read_exact(&mut body)?;
        body.truncate(n);
        Ok(body)
    }
}

fn number(b: &[u8]) -> io::Result<i64> {
    String::from_utf8_lossy(b)
        .trim()
        .parse()
        .map_err(|_| io::Error::other(format!("not a length: {:?}", show(b))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_spellings_of_nothing_stay_apart() {
        assert_ne!(Reply::Null.render(), Reply::NullBulk.render());
        assert_ne!(Reply::NullBulk.render(), Reply::NullArray.render());
    }

    #[test]
    fn a_bulk_and_a_verbatim_carrying_the_same_bytes_are_not_the_same_reply() {
        let bulk = Reply::Bulk(b"txt:hello".to_vec());
        let verbatim = Reply::Verbatim(b"txt:hello".to_vec());
        assert_ne!(bulk.render(), verbatim.render());
    }

    #[test]
    fn the_first_word_of_an_error_is_the_part_that_has_to_match() {
        let e = Reply::Error(b"WRONGTYPE Operation against a key".to_vec());
        assert_eq!(e.error_word().as_deref(), Some("WRONGTYPE"));
        assert_eq!(Reply::Simple(b"OK".to_vec()).error_word(), None);
    }
}
