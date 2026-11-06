use crate::Error;

const MAX_BLOB: usize = 8 * 1024 * 1024;

pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    pub fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Length-prefixed UTF-8. The length is the byte count, little-endian.
    pub fn str(&mut self, value: &str) -> Result<(), Error> {
        let len = u16::try_from(value.len()).map_err(|_| Error::FieldTooLong)?;
        self.buf.extend_from_slice(&len.to_le_bytes());
        self.buf.extend_from_slice(value.as_bytes());
        Ok(())
    }

    pub fn blob(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > MAX_BLOB {
            return Err(Error::FieldTooLong);
        }
        let len = u32::try_from(bytes.len()).map_err(|_| Error::FieldTooLong)?;
        self.u32(len);
        self.bytes(bytes);
        Ok(())
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    index: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, index: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.index.checked_add(len).ok_or(Error::Truncated)?;
        if end > self.buf.len() {
            return Err(Error::Truncated);
        }
        let out = &self.buf[self.index..end];
        self.index = end;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    pub fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let bytes = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    pub fn str(&mut self) -> Result<String, Error> {
        let len_bytes = self.take(2)?;
        let len = u16::from_le_bytes(len_bytes.try_into().expect("2 bytes")) as usize;
        let bytes = self.take(len)?;
        let text = std::str::from_utf8(bytes).map_err(|_| Error::BadString)?;
        Ok(text.to_owned())
    }

    pub fn blob(&mut self) -> Result<Vec<u8>, Error> {
        let len = self.u32()? as usize;
        if len > MAX_BLOB {
            return Err(Error::FieldTooLong);
        }
        Ok(self.take(len)?.to_vec())
    }

    pub fn expect_magic(&mut self, magic: &[u8; 4], version: u8) -> Result<(), Error> {
        let got = self.array::<4>()?;
        if got != *magic {
            return Err(Error::BadMagic);
        }
        let got_version = self.u8()?;
        if got_version != version {
            return Err(Error::BadVersion);
        }
        Ok(())
    }

    pub fn finish(self) -> Result<(), Error> {
        if self.index != self.buf.len() {
            Err(Error::TrailingBytes)
        } else {
            Ok(())
        }
    }
}

pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
