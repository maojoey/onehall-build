//! Model-facing input/output tools. Identity is derived from the bound
//! connection and the task's current run; the model supplies only an object or
//! slot id, never a task, attempt, run, address or path.
use super::{mailbox::denied, TeamService};
use crate::{
    app_error::AppCommandError,
    hall::{
        exchange::{
            runtime::current_owner, AttemptOwner, ExchangeError, ExitVerification, ObjectRef,
        },
        tasks::runtime::authority::Actor,
    },
};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};

pub(super) const DEFAULT_READ_BYTES: u64 = 16 * 1024;
pub(super) const MAX_READ_BYTES: u64 = 64 * 1024;

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct TextChunk {
    pub text: String,
    pub next_offset: u64,
    pub eof: bool,
    pub truncated: bool,
}

fn invalid(message: &str) -> AppCommandError {
    AppCommandError::invalid_input(message)
}

/// Validate the entire object, not only the requested window. Keep at most
/// three trailing UTF-8 bytes between bounded reads; no file-sized allocation.
fn validate_text<R: Read + Seek>(reader: &mut R, size: u64) -> Result<(), AppCommandError> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| invalid("input could not be read"))?;
    let mut buffer = vec![0u8; 65536 + 3];
    let mut remaining = size;
    let mut carry = 0usize;
    while remaining > 0 {
        let count = remaining.min(65536) as usize;
        reader
            .read_exact(&mut buffer[carry..carry + count])
            .map_err(|_| invalid("input could not be read"))?;
        remaining -= count as u64;
        let len = carry + count;
        if buffer[..len].contains(&0) {
            return Err(invalid("input is binary, not text"));
        }
        match std::str::from_utf8(&buffer[..len]) {
            Ok(_) => carry = 0,
            Err(error) if error.error_len().is_none() && remaining > 0 => {
                let valid = error.valid_up_to();
                carry = len - valid;
                buffer.copy_within(valid..len, 0);
            }
            Err(_) => return Err(invalid("input is not UTF-8 text")),
        }
    }
    if carry != 0 {
        return Err(invalid("input is not UTF-8 text"));
    }
    Ok(())
}

/// Decode one window of a file. `offset` must sit on a character boundary; the
/// end is pulled back to the last complete character. A chunk that is not
/// UTF-8 text (NUL or invalid sequences) is refused rather than guessed at.
pub(super) fn text_chunk<R: Read + Seek>(
    reader: &mut R,
    size: u64,
    offset: u64,
    max_bytes: Option<u64>,
) -> Result<TextChunk, AppCommandError> {
    let limit = max_bytes.unwrap_or(DEFAULT_READ_BYTES);
    if limit == 0 || limit > MAX_READ_BYTES {
        return Err(invalid("maxBytes must be between 1 and 65536"));
    }
    if offset > size {
        return Err(invalid("offset is beyond the end of the input"));
    }
    validate_text(reader, size)?;
    if offset == size {
        return Ok(TextChunk {
            text: String::new(),
            next_offset: size,
            eof: true,
            truncated: false,
        });
    }
    let want = limit.min(size - offset);
    let mut window = vec![0u8; want as usize];
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|_| invalid("input could not be read"))?;
    reader
        .read_exact(&mut window)
        .map_err(|_| invalid("input could not be read"))?;
    if window[0] & 0xC0 == 0x80 {
        return Err(invalid(
            "offset is inside a character; continue from a returned nextOffset",
        ));
    }
    let at_eof = offset + want == size;
    let end = match std::str::from_utf8(&window) {
        Ok(_) => window.len(),
        // A character cut by the window is resumed; one cut by EOF is invalid.
        Err(error) if error.error_len().is_none() && !at_eof => error.valid_up_to(),
        Err(_) => return Err(invalid("input is not UTF-8 text")),
    };
    if end == 0 {
        return Err(invalid("maxBytes is too small for the next character"));
    }
    let text =
        std::str::from_utf8(&window[..end]).map_err(|_| invalid("input is not UTF-8 text"))?;
    if text.contains('\0') {
        return Err(invalid("input is binary, not text"));
    }
    let next_offset = offset + end as u64;
    let eof = next_offset >= size;
    Ok(TextChunk {
        text: text.to_owned(),
        next_offset,
        eof,
        truncated: !eof,
    })
}

fn exchange_error(error: ExchangeError) -> AppCommandError {
    // Exchange errors stay opaque to the model except for fixable input.
    match error {
        ExchangeError::Invalid => invalid("object or slot id is invalid"),
        ExchangeError::Quota => invalid("output exceeds the exchange size limit"),
        ExchangeError::Unstable => invalid("output file changed while it was being sealed; retry"),
        _ => denied(),
    }
}

impl TeamService {
    /// The task's current attached run, checked against the connection's own
    /// attempt so a bound-but-superseded actor cannot act for a newer run.
    async fn attached_run(
        &self,
        a: &Actor,
    ) -> Result<(AttemptOwner, ExitVerification), AppCommandError> {
        let (_, task, attempt) = self.own(a).await?;
        let (owner, exit) = current_owner(self.host.database(), &task)
            .await
            .map_err(|_| denied())?
            .ok_or_else(denied)?;
        if !owner.is_attempt(&attempt) {
            return Err(denied());
        }
        Ok((owner, exit))
    }
    pub(super) async fn list_inputs(&self, a: &Actor) -> Result<Vec<ObjectRef>, AppCommandError> {
        let exchange = self.exchange.get().ok_or_else(denied)?;
        let (owner, _) = self.attached_run(a).await?;
        exchange.list_inputs(&owner).await.map_err(exchange_error)
    }
    pub(super) async fn read_input(
        &self,
        a: &Actor,
        object: &str,
        offset: u64,
        max_bytes: Option<u64>,
    ) -> Result<TextChunk, AppCommandError> {
        let exchange = self.exchange.get().ok_or_else(denied)?;
        let (owner, _) = self.attached_run(a).await?;
        let file = exchange
            .open_input(&owner, object)
            .await
            .map_err(exchange_error)?;
        tokio::task::spawn_blocking(move || {
            let mut file = file;
            let size = file.metadata().map_err(|_| denied())?.len();
            text_chunk(&mut file, size, offset, max_bytes)
        })
        .await
        .map_err(|_| denied())?
    }
    pub(super) async fn submit_output(
        &self,
        a: &Actor,
        slot: &str,
    ) -> Result<Value, AppCommandError> {
        let exchange = self.exchange.get().ok_or_else(denied)?;
        let (owner, exit) = self.attached_run(a).await?;
        let artifact = exchange
            .seal(&owner, slot, exit)
            .await
            .map_err(exchange_error)?;
        serde_json::to_value(artifact).map_err(|_| denied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn chunk(bytes: &[u8], offset: u64, max: u64) -> Result<TextChunk, AppCommandError> {
        text_chunk(
            &mut Cursor::new(bytes.to_vec()),
            bytes.len() as u64,
            offset,
            Some(max),
        )
    }
    #[test]
    fn chinese_text_is_cut_on_character_boundaries_and_resumes() {
        let text = "一堂文件中转";
        let bytes = text.as_bytes();
        let first = chunk(bytes, 0, 7).unwrap();
        assert_eq!(first.text, "一堂");
        assert_eq!(first.next_offset, 6);
        assert!(first.truncated && !first.eof);
        let rest = chunk(bytes, first.next_offset, 65536).unwrap();
        assert_eq!(rest.text, "文件中转");
        assert!(rest.eof && !rest.truncated);
        assert_eq!(format!("{}{}", first.text, rest.text), text);
    }
    #[test]
    fn eof_empty_read_and_bad_offsets() {
        let bytes = "abc".as_bytes();
        let eof = chunk(bytes, 3, 10).unwrap();
        assert!(eof.eof && eof.text.is_empty() && eof.next_offset == 3);
        assert!(chunk(bytes, 4, 10).is_err());
        // Inside the 3-byte character 一.
        assert!(chunk("一".as_bytes(), 1, 10).is_err());
        assert!(chunk(bytes, 0, 0).is_err());
        assert!(chunk(bytes, 0, MAX_READ_BYTES + 1).is_err());
        // A window smaller than the next character cannot make progress.
        assert!(chunk("一".as_bytes(), 0, 2).is_err());
    }
    #[test]
    fn binary_and_invalid_utf8_are_refused_not_called_text() {
        assert!(chunk(b"ab\0cd", 0, 10).is_err());
        assert!(chunk(&[0xff, 0xfe, 0x00], 0, 10).is_err());
        assert!(chunk(&[b'a', 0xc3, b'a'], 0, 10).is_err());
        // A file ending inside a character is not text, not a too-small window.
        let cut = &"a一".as_bytes()[..3];
        let error = chunk(cut, 0, 10).unwrap_err();
        assert!(format!("{error:?}").contains("not UTF-8"), "{error:?}");
        assert!(format!("{:?}", chunk(cut, 1, 10).unwrap_err()).contains("not UTF-8"));
        // A binary region outside the requested window still makes the file binary.
        let mixed = [b"text".as_slice(), &[0u8; 4]].concat();
        assert!(chunk(&mixed, 0, 4).is_err());
        assert!(chunk(&mixed, mixed.len() as u64, 4).is_err());
        let mut crossing = vec![b'a'; 65535];
        crossing.extend_from_slice("一".as_bytes());
        assert!(chunk(&crossing, 0, 4).is_ok());
        crossing.push(0xff);
        assert!(chunk(&crossing, 0, 4).is_err());
    }
    #[test]
    fn a_truncated_read_is_never_marked_complete() {
        let bytes = vec![b'a'; 20_000];
        let first = text_chunk(&mut Cursor::new(bytes.clone()), 20_000, 0, None).unwrap();
        assert_eq!(first.text.len() as u64, DEFAULT_READ_BYTES);
        assert!(first.truncated && !first.eof);
        let last = text_chunk(&mut Cursor::new(bytes), 20_000, first.next_offset, None).unwrap();
        assert!(last.eof && !last.truncated);
    }
}
