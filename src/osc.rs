//! Spots, in the shell's output, the marks emitted by the shell integration.
//!
//! The stream is split into segments: ordinary bytes, meant for the
//! emulator, and marks, removed from the stream. A mark may arrive cut
//! between two PTY reads: the incomplete tail is then kept aside until the
//! next read.

/// An event reported by the shell integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// `OSC 133 ; A` — a new prompt starts.
    PromptStart,
    /// `OSC 133 ; L` — the command line was accepted.
    LineFinish,
    /// `OSC 133 ; C` — the command starts, its output follows.
    CommandStart,
    /// `OSC 133 ; D ; code` — the command finished.
    CommandEnd(Option<i32>),
    /// `OSC 1337 ; CurrentDir=path` — the shell's current directory.
    Cwd(String),
    /// `OSC 1337 ; RTermPath=…` — the shell's `PATH`, for completion.
    Path(String),
    /// `OSC 1337 ; RTermCommands=…` — the shell's aliases, functions and
    /// builtins, separated by spaces.
    Commands(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Bytes(&'a [u8]),
    Mark(Mark),
}

const SEMANTIC_PREFIX: &[u8] = b"\x1b]133;";
const CWD_PREFIX: &[u8] = b"\x1b]1337;CurrentDir=";
const PATH_PREFIX: &[u8] = b"\x1b]1337;RTermPath=";
const COMMANDS_PREFIX: &[u8] = b"\x1b]1337;RTermCommands=";
const PREFIXES: [&[u8]; 4] = [SEMANTIC_PREFIX, CWD_PREFIX, PATH_PREFIX, COMMANDS_PREFIX];

/// Beyond this size, an unterminated sequence is no longer a mark.
/// (A well-stocked `PATH` easily exceeds a few kilobytes.)
const MAX_MARK_LEN: usize = 64 * 1024;

#[derive(Default)]
pub struct Scanner {
    carry: Vec<u8>,
}

impl Scanner {
    pub fn feed(&mut self, chunk: &[u8], mut emit: impl FnMut(Segment<'_>)) {
        let owned;
        let data: &[u8] = if self.carry.is_empty() {
            chunk
        } else {
            let mut joined = std::mem::take(&mut self.carry);
            joined.extend_from_slice(chunk);
            owned = joined;
            &owned
        };

        let mut plain_start = 0;
        let mut search_from = 0;
        while let Some(offset) = data[search_from..].iter().position(|&byte| byte == 0x1b) {
            let escape = search_from + offset;
            let rest = &data[escape..];

            match Self::parse(rest) {
                Parsed::Mark { mark, len } => {
                    if escape > plain_start {
                        emit(Segment::Bytes(&data[plain_start..escape]));
                    }
                    if let Some(mark) = mark {
                        emit(Segment::Mark(mark));
                    }
                    plain_start = escape + len;
                    search_from = plain_start;
                }
                Parsed::Incomplete => {
                    if escape > plain_start {
                        emit(Segment::Bytes(&data[plain_start..escape]));
                    }
                    self.carry = rest.to_vec();
                    return;
                }
                Parsed::NotAMark => search_from = escape + 1,
            }
        }

        if plain_start < data.len() {
            emit(Segment::Bytes(&data[plain_start..]));
        }
    }

    /// Parses what starts with an Escape.
    fn parse(rest: &[u8]) -> Parsed {
        for prefix in PREFIXES {
            if rest.len() < prefix.len() {
                if prefix.starts_with(rest) {
                    return Parsed::Incomplete;
                }
                continue;
            }
            if !rest.starts_with(prefix) {
                continue;
            }

            let payload = &rest[prefix.len()..];
            // An OSC sequence ends with BEL or with `ESC \`.
            let end = payload.iter().enumerate().find_map(|(index, &byte)| match byte {
                0x07 => Some((index, 1)),
                0x1b if payload.get(index + 1) == Some(&b'\\') => Some((index, 2)),
                _ => None,
            });
            return match end {
                Some((index, terminator)) => Parsed::Mark {
                    mark: Self::mark(prefix, &payload[..index]),
                    len: prefix.len() + index + terminator,
                },
                None if rest.len() > MAX_MARK_LEN => Parsed::NotAMark,
                None => Parsed::Incomplete,
            };
        }
        Parsed::NotAMark
    }

    fn mark(prefix: &[u8], payload: &[u8]) -> Option<Mark> {
        let payload = String::from_utf8_lossy(payload);
        if prefix == CWD_PREFIX {
            return Some(Mark::Cwd(payload.into_owned()));
        }
        if prefix == PATH_PREFIX {
            return Some(Mark::Path(payload.into_owned()));
        }
        if prefix == COMMANDS_PREFIX {
            return Some(Mark::Commands(payload.into_owned()));
        }
        let mut fields = payload.split(';');
        match fields.next()? {
            "A" => Some(Mark::PromptStart),
            "L" => Some(Mark::LineFinish),
            "C" => Some(Mark::CommandStart),
            "D" => Some(Mark::CommandEnd(fields.next().and_then(|code| code.parse().ok()))),
            // Marks RTerm does not use (B, for instance): ignored.
            _ => None,
        }
    }
}

enum Parsed {
    /// A complete sequence; `mark` is `None` when it should be ignored.
    Mark { mark: Option<Mark>, len: usize },
    Incomplete,
    NotAMark,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits `input` into chunks of `chunk_size` bytes and gathers the result.
    fn scan(input: &[u8], chunk_size: usize) -> (Vec<u8>, Vec<Mark>) {
        let mut scanner = Scanner::default();
        let (mut bytes, mut marks) = (Vec::new(), Vec::new());
        for chunk in input.chunks(chunk_size) {
            scanner.feed(chunk, |segment| match segment {
                Segment::Bytes(b) => bytes.extend_from_slice(b),
                Segment::Mark(mark) => marks.push(mark),
            });
        }
        (bytes, marks)
    }

    const STREAM: &[u8] =
        b"output\x1b]133;D;127\x07\x1b]1337;CurrentDir=/tmp/a b\x07\x1b]1337;RTermPath=/bin:/usr/bin\x07\x1b]133;A\x1b\\\x1b[1mprompt\x1b]133;C\x07end";

    #[test]
    fn marks_are_extracted_and_removed() {
        let (bytes, marks) = scan(STREAM, STREAM.len());
        assert_eq!(bytes, b"output\x1b[1mpromptend");
        assert_eq!(
            marks,
            vec![
                Mark::CommandEnd(Some(127)),
                Mark::Cwd("/tmp/a b".into()),
                Mark::Path("/bin:/usr/bin".into()),
                Mark::PromptStart,
                Mark::CommandStart,
            ]
        );
    }

    #[test]
    fn marks_split_across_reads_are_reassembled() {
        let whole = scan(STREAM, STREAM.len());
        for chunk_size in 1..STREAM.len() {
            assert_eq!(scan(STREAM, chunk_size), whole, "chunks of {chunk_size}");
        }
    }

    #[test]
    fn other_escape_sequences_pass_through() {
        let input = b"\x1b]0;title\x07\x1b[31mred\x1b]133;B\x07";
        let (bytes, marks) = scan(input, 3);
        assert_eq!(bytes, b"\x1b]0;title\x07\x1b[31mred");
        assert!(marks.is_empty());
    }
}
