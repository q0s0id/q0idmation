use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnexpectedEof {
        offset: usize,
        needed: usize,
        available: usize,
    },
    InvalidMagic([u8; 4]),
    UnsupportedVersion(u16),
    UnsupportedFlags(u16),
    TrailingBytes {
        offset: usize,
        remaining: usize,
    },
    InvalidUtf8,
    InvalidTag(u8),
    InvalidRecord {
        tag: u8,
        reason: &'static str,
    },
    InvalidAssetKind(u8),
    Validation(&'static str),
    Overflow(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof {
                offset,
                needed,
                available,
            } => write!(
                f,
                "unexpected EOF at offset {}, needed {}, available {}",
                offset, needed, available
            ),
            Self::InvalidMagic(magic) => write!(f, "invalid magic: {:?}", magic),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version {}", v),
            Self::UnsupportedFlags(flags) => {
                write!(f, "unsupported reserved flags 0x{flags:04x}")
            }
            Self::TrailingBytes { offset, remaining } => write!(
                f,
                "trailing bytes at offset {offset}: {remaining} byte(s) remain"
            ),
            Self::InvalidUtf8 => write!(f, "invalid utf-8 in string field"),
            Self::InvalidTag(tag) => write!(f, "invalid tag {}", tag),
            Self::InvalidRecord { tag, reason } => {
                write!(f, "invalid record for tag {}: {}", tag, reason)
            }
            Self::InvalidAssetKind(kind) => write!(f, "invalid asset kind {}", kind),
            Self::Validation(msg) => write!(f, "validation failed: {}", msg),
            Self::Overflow(what) => write!(f, "overflow: {}", what),
        }
    }
}

impl std::error::Error for Error {}
