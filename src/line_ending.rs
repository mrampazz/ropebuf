use std::io::{self, Read, Seek, SeekFrom, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// \n
    LF,
    /// \r\n
    CRLF,
    /// \r (leg
    CR,
    /// \u{0085} (Next Line)
    NEL,
    /// \u{2028} (Unicode Line Separator)
    LS,
    /// \u{2029} (Unicode Paragraph Separator)
    PS,
}

impl LineEnding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LF => "\n",
            Self::CRLF => "\r\n",
            Self::CR => "\r",
            Self::NEL => "\u{0085}",
            Self::LS => "\u{2028}",
            Self::PS => "\u{2029}",
        }
    }

    fn as_bytes(&self) -> &'static [u8] {
        self.as_str().as_bytes()
    }
}

impl Default for LineEnding {
    fn default() -> Self {
        Self::LF
    }
}

pub fn detect_line_ending<R: Read + Seek>(reader: &mut R) -> LineEnding {
    use LineEnding::*;
    let mut buffer = [0u8; 4096];
    let n = reader.read(&mut buffer).unwrap_or(0);
    let _ = reader.seek(SeekFrom::Start(0));

    let kinds = [CRLF, CR, NEL, LS, PS, LF];

    let mut counts = [0usize; 6];
    let mut i = 0;
    while i < n {
        let rest = &buffer[i..n];
        let matched = kinds
            .iter()
            .enumerate()
            .find(|(_, kind)| rest.starts_with(kind.as_bytes()));
        match matched {
            Some((idx, kind)) => {
                counts[idx] += 1;
                i += kind.as_bytes().len();
            }
            None => i += 1,
        }
    }
    let (best_idx, &best_count) = counts
        .iter()
        .enumerate()
        .max_by_key(|&(_, &count)| count)
        .unwrap();

    if best_count == 0 { LF } else { kinds[best_idx] }
}

/// ropebuf normalises to LF so the editor only has to deal with LF line ending
pub struct LineNormaliser<R: Read> {
    inner: R,
    pattern: &'static [u8],
    pending: Vec<u8>,
    eof: bool,
}

impl<R: Read> LineNormaliser<R> {
    pub fn new(inner: R, ending: LineEnding) -> Self {
        Self {
            inner,
            pattern: ending.as_bytes(),
            pending: Vec::new(),
            eof: false,
        }
    }
}

impl<R: Read> Read for LineNormaliser<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pattern == b"\n" {
            return self.inner.read(buf);
        }

        loop {
            let mut chunk = std::mem::take(&mut self.pending);
            if !self.eof {
                let mut temp = vec![0u8; buf.len().max(self.pattern.len())];
                let n = self.inner.read(&mut temp)?;
                if n == 0 {
                    self.eof = true;
                } else {
                    chunk.extend_from_slice(&temp[..n]);
                }
            }
            let mut out_idx = 0;
            let mut i = 0;
            while i < chunk.len() {
                if out_idx >= buf.len() {
                    self.pending = chunk[i..].to_vec();
                    return Ok(out_idx);
                }
                let remaining = chunk.len() - i;
                if remaining >= self.pattern.len()
                    && chunk[i..i + self.pattern.len()] == *self.pattern
                {
                    buf[out_idx] = b'\n';
                    out_idx += 1;
                    i += self.pattern.len();
                } else if !self.eof
                    && remaining < self.pattern.len()
                    && chunk[i..] == self.pattern[..remaining]
                {
                    self.pending = chunk[i..].to_vec();
                    i = chunk.len();
                } else {
                    buf[out_idx] = chunk[i];
                    out_idx += 1;
                    i += 1;
                }
            }
            if out_idx > 0 || self.eof {
                return Ok(out_idx);
            }
        }
    }
}

pub struct LineEndingWriter<W: Write> {
    inner: W,
    ending: LineEnding,
}

impl<W: Write> LineEndingWriter<W> {
    pub fn new(inner: W, ending: LineEnding) -> Self {
        Self { inner, ending }
    }
}

impl<W: Write> Write for LineEndingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.ending == LineEnding::LF {
            return self.inner.write(buf);
        }

        for &byte in buf {
            if byte == b'\n' {
                self.inner.write_all(self.ending.as_bytes())?;
            } else {
                self.inner.write_all(&[byte])?;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn detect(bytes: &[u8]) -> LineEnding {
        detect_line_ending(&mut Cursor::new(bytes.to_vec()))
    }

    fn normalize(bytes: &[u8], ending: LineEnding) -> Vec<u8> {
        let mut normalizer = LineNormaliser::new(Cursor::new(bytes.to_vec()), ending);
        let mut out = Vec::new();
        normalizer.read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn detects_each_common_and_unicode_ending() {
        assert_eq!(detect(b"a\nb\nc"), LineEnding::LF);
        assert_eq!(detect(b"a\r\nb\r\nc"), LineEnding::CRLF);
        assert_eq!(detect(b"a\rb\rc"), LineEnding::CR);
        assert_eq!(detect("a\u{0085}b\u{0085}c".as_bytes()), LineEnding::NEL);
        assert_eq!(detect("a\u{2028}b\u{2028}c".as_bytes()), LineEnding::LS);
        assert_eq!(detect("a\u{2029}b\u{2029}c".as_bytes()), LineEnding::PS);
        assert_eq!(detect(b"no newlines here"), LineEnding::LF);
    }

    #[test]
    fn normalizer_collapses_every_ending_to_lf() {
        assert_eq!(normalize(b"a\r\nb\r\nc", LineEnding::CRLF), b"a\nb\nc");
        assert_eq!(normalize(b"a\rb\rc", LineEnding::CR), b"a\nb\nc");
        assert_eq!(
            normalize("a\u{2028}b\u{2028}c".as_bytes(), LineEnding::LS),
            b"a\nb\nc"
        );
    }

    #[test]
    fn normalizer_handles_multi_byte_pattern_split_across_reads() {
        struct OneByteAtATime(std::vec::IntoIter<u8>);
        impl Read for OneByteAtATime {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                match self.0.next() {
                    Some(b) => {
                        buf[0] = b;
                        Ok(1)
                    }
                    None => Ok(0),
                }
            }
        }

        let input = "a\u{2028}b".as_bytes().to_vec();
        let mut normalizer = LineNormaliser::new(OneByteAtATime(input.into_iter()), LineEnding::LS);
        let mut out = Vec::new();
        normalizer.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"a\nb");
    }

    #[test]
    fn writer_round_trips_each_ending() {
        for ending in [
            LineEnding::LF,
            LineEnding::CRLF,
            LineEnding::CR,
            LineEnding::NEL,
            LineEnding::LS,
            LineEnding::PS,
        ] {
            let mut out = Vec::new();
            {
                let mut writer = LineEndingWriter::new(&mut out, ending);
                writer.write_all(b"a\nb\nc").unwrap();
            }
            let normalized = normalize(&out, ending);
            assert_eq!(normalized, b"a\nb\nc");
        }
    }
}
