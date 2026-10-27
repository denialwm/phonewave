//! The protobuf wire format, as much as the sensor core uses: varints,
//! fixed 32 and 64-bit fields, and length-delimited bytes.

/// Builds one message, field by field, in the order written.
#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(&mut self, field: u32, wire: u8) {
        self.varint_raw(u64::from(field) << 3 | u64::from(wire));
    }

    fn varint_raw(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.0.push(v as u8 | 0x80);
            v >>= 7;
        }
        self.0.push(v as u8);
    }

    pub fn varint(mut self, field: u32, v: u64) -> Self {
        self.key(field, 0);
        self.varint_raw(v);
        self
    }

    pub fn bool(self, field: u32, v: bool) -> Self {
        self.varint(field, v.into())
    }

    pub fn fixed32(mut self, field: u32, v: u32) -> Self {
        self.key(field, 5);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn float(self, field: u32, v: f32) -> Self {
        self.fixed32(field, v.to_bits())
    }

    pub fn fixed64(mut self, field: u32, v: u64) -> Self {
        self.key(field, 1);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bytes(mut self, field: u32, v: &[u8]) -> Self {
        self.key(field, 2);
        self.varint_raw(v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }

    pub fn message(self, field: u32, m: Writer) -> Self {
        self.bytes(field, &m.0)
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// One field's value as it travels; its meaning is up to the reader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Value<'a> {
    pub fn as_u64(self) -> Option<u64> {
        match self {
            Value::Varint(v) | Value::Fixed64(v) => Some(v),
            Value::Fixed32(v) => Some(v.into()),
            Value::Bytes(_) => None,
        }
    }

    pub fn as_f32(self) -> Option<f32> {
        match self {
            Value::Fixed32(v) => Some(f32::from_bits(v)),
            _ => None,
        }
    }

    pub fn as_bytes(self) -> Option<&'a [u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(self) -> Option<&'a str> {
        std::str::from_utf8(self.as_bytes()?).ok()
    }

    /// The floats of a repeated float field, packed or not: a packed field
    /// arrives as bytes, an unpacked one as one fixed32 per element.
    pub fn floats(self) -> Vec<f32> {
        match self {
            Value::Fixed32(v) => vec![f32::from_bits(v)],
            Value::Bytes(b) => b
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// The fields of one message in wire order. Stops at the first malformed
/// field; `fields` then ends early.
pub struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = self.buf.split_first()?;
            self.buf = rest;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.buf.len() < n {
            return None;
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Some(head)
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = (u32, Value<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.buf.is_empty() {
            return None;
        }
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).ok()?;
        let value = match key & 7 {
            0 => Value::Varint(self.varint()?),
            1 => Value::Fixed64(u64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            2 => {
                let n = usize::try_from(self.varint()?).ok()?;
                Value::Bytes(self.take(n)?)
            }
            5 => Value::Fixed32(u32::from_le_bytes(self.take(4)?.try_into().unwrap())),
            _ => {
                self.buf = &[];
                return None;
            }
        };
        Some((field, value))
    }
}

/// The first value of `field`, if the message has one.
pub fn field(buf: &[u8], field: u32) -> Option<Value<'_>> {
    Reader::new(buf).find(|(f, _)| *f == field).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_wire_type() {
        let inner = Writer::new().fixed64(1, 0xabab_abab_abab_abab);
        let buf = Writer::new()
            .varint(1, 300)
            .bool(2, true)
            .float(3, 1.5)
            .fixed64(4, 7)
            .bytes(5, b"accel")
            .message(6, inner)
            .finish();
        let fields: Vec<_> = Reader::new(&buf).collect();
        assert_eq!(fields[0], (1, Value::Varint(300)));
        assert_eq!(fields[1], (2, Value::Varint(1)));
        assert_eq!(fields[2].1.as_f32(), Some(1.5));
        assert_eq!(fields[3], (4, Value::Fixed64(7)));
        assert_eq!(fields[4].1.as_str(), Some("accel"));
        let nested = fields[5].1.as_bytes().unwrap();
        assert_eq!(
            field(nested, 1),
            Some(Value::Fixed64(0xabab_abab_abab_abab))
        );
    }

    #[test]
    fn reads_packed_and_unpacked_floats() {
        let mut packed = Vec::new();
        for f in [1.0f32, -2.0, 9.5] {
            packed.extend_from_slice(&f.to_le_bytes());
        }
        assert_eq!(Value::Bytes(&packed).floats(), [1.0, -2.0, 9.5]);
        assert_eq!(Value::Fixed32(3.0f32.to_bits()).floats(), [3.0]);
    }

    #[test]
    fn stops_at_truncated_fields() {
        // Field 1, length 5, but only two bytes follow.
        assert_eq!(Reader::new(&[0x0a, 5, 1, 2]).count(), 0);
        assert_eq!(Reader::new(&[0x08]).count(), 0);
    }
}
