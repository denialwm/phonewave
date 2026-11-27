//! The first video track of an MP4 file: its codec, size, frame rate, and
//! where each sample lies, read from the `moov` box. Samples are read from
//! the file one at a time and rewritten as an Annex B stream, which the
//! V4L2 decoders take.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

pub struct Track {
    file: File,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    /// The mean sample duration.
    pub frame_interval: Duration,
    /// Sample offsets and sizes in the file, in decode order.
    samples: Vec<(u64, u32)>,
    /// The largest sample.
    pub max_sample: usize,
    /// Bytes in each NAL unit's length prefix.
    length_size: usize,
    /// The parameter sets, as Annex B.
    pub parameter_sets: Vec<u8>,
}

const START_CODE: [u8; 4] = [0, 0, 0, 1];

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// The boxes in `data`: (type, body).
fn boxes(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        if data.len() < 8 {
            return None;
        }
        let size = u32::from_be_bytes(data[0..4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = data[4..8].try_into().unwrap();
        let (header, size) = match size {
            0 => (8, data.len() as u64),
            1 if data.len() >= 16 => (16, u64::from_be_bytes(data[8..16].try_into().unwrap())),
            _ => (8, size),
        };
        if size < header || size > data.len() as u64 {
            return None;
        }
        let body = &data[header as usize..size as usize];
        data = &data[size as usize..];
        Some((kind, body))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| k == kind).map(|(_, body)| body)
}

fn descend<'a>(data: &'a [u8], kinds: &[&[u8; 4]]) -> Option<&'a [u8]> {
    kinds.iter().try_fold(data, |data, kind| child(data, kind))
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(invalid("box ends early"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// The `moov` box's body, read from the file's top level.
fn read_moov(file: &File) -> io::Result<Vec<u8>> {
    let length = file.metadata()?.len();
    let mut offset = 0u64;
    while offset + 8 <= length {
        let mut header = [0u8; 16];
        file.read_exact_at(&mut header[..8], offset)?;
        let mut size = u32::from_be_bytes(header[0..4].try_into().unwrap()) as u64;
        let mut header_size = 8;
        if size == 1 {
            file.read_exact_at(&mut header[8..16], offset + 8)?;
            size = u64::from_be_bytes(header[8..16].try_into().unwrap());
            header_size = 16;
        } else if size == 0 {
            size = length - offset;
        }
        if size < header_size {
            break;
        }
        if &header[4..8] == b"moov" {
            let mut body = vec![0u8; (size - header_size) as usize];
            file.read_exact_at(&mut body, offset + header_size)?;
            return Ok(body);
        }
        offset += size;
    }
    Err(invalid("no moov box"))
}

/// Length-prefixed parameter sets as Annex B.
fn annex_b_sets(reader: &mut Reader, count: usize, out: &mut Vec<u8>) -> io::Result<()> {
    for _ in 0..count {
        let length = reader.u16()? as usize;
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(reader.take(length)?);
    }
    Ok(())
}

/// The codec, length prefix size and parameter sets of a sample entry.
fn sample_entry(stsd: &[u8]) -> io::Result<(Codec, u32, u32, usize, Vec<u8>)> {
    // Version and flags, entry count, then the first entry.
    let (kind, entry) = boxes(stsd.get(8..).ok_or_else(|| invalid("short stsd"))?)
        .next()
        .ok_or_else(|| invalid("empty stsd"))?;
    let codec = match &kind {
        b"avc1" | b"avc3" => Codec::H264,
        b"hvc1" | b"hev1" => Codec::Hevc,
        other => {
            return Err(invalid(format!(
                "unsupported video codec {}",
                String::from_utf8_lossy(other)
            )));
        }
    };
    // A visual sample entry's fixed part is 78 bytes.
    let mut fixed = Reader(
        entry
            .get(..78)
            .ok_or_else(|| invalid("short sample entry"))?,
    );
    fixed.take(24)?;
    let width = fixed.u16()? as u32;
    let height = fixed.u16()? as u32;
    let children = &entry[78..];
    let mut sets = Vec::new();
    let length_size = match codec {
        Codec::H264 => {
            let mut avcc = Reader(child(children, b"avcC").ok_or_else(|| invalid("no avcC"))?);
            avcc.take(4)?;
            let length_size = (avcc.u8()? & 3) as usize + 1;
            let sps = (avcc.u8()? & 0x1f) as usize;
            annex_b_sets(&mut avcc, sps, &mut sets)?;
            let pps = avcc.u8()? as usize;
            annex_b_sets(&mut avcc, pps, &mut sets)?;
            length_size
        }
        Codec::Hevc => {
            let mut hvcc = Reader(child(children, b"hvcC").ok_or_else(|| invalid("no hvcC"))?);
            hvcc.take(21)?;
            let length_size = (hvcc.u8()? & 3) as usize + 1;
            let arrays = hvcc.u8()?;
            for _ in 0..arrays {
                hvcc.u8()?;
                let count = hvcc.u16()? as usize;
                annex_b_sets(&mut hvcc, count, &mut sets)?;
            }
            length_size
        }
    };
    Ok((codec, width, height, length_size, sets))
}

/// Each sample's offset and size, from the sample table.
fn sample_locations(stbl: &[u8]) -> io::Result<Vec<(u64, u32)>> {
    let mut stsz = Reader(child(stbl, b"stsz").ok_or_else(|| invalid("no stsz"))?);
    stsz.take(4)?;
    let uniform = stsz.u32()?;
    let count = stsz.u32()? as usize;
    let sizes = (0..count)
        .map(|_| {
            if uniform != 0 {
                Ok(uniform)
            } else {
                stsz.u32()
            }
        })
        .collect::<io::Result<Vec<u32>>>()?;

    let chunks = if let Some(stco) = child(stbl, b"stco") {
        let mut stco = Reader(stco);
        stco.take(4)?;
        let n = stco.u32()?;
        (0..n)
            .map(|_| stco.u32().map(u64::from))
            .collect::<io::Result<Vec<_>>>()?
    } else {
        let mut co64 = Reader(child(stbl, b"co64").ok_or_else(|| invalid("no chunk offsets"))?);
        co64.take(4)?;
        let n = co64.u32()?;
        (0..n).map(|_| co64.u64()).collect::<io::Result<Vec<_>>>()?
    };

    let mut stsc = Reader(child(stbl, b"stsc").ok_or_else(|| invalid("no stsc"))?);
    stsc.take(4)?;
    let runs = (0..stsc.u32()?)
        .map(|_| {
            let first = stsc.u32()?;
            let per_chunk = stsc.u32()?;
            stsc.u32()?;
            Ok((first as usize, per_chunk as usize))
        })
        .collect::<io::Result<Vec<_>>>()?;

    let mut locations = Vec::with_capacity(count);
    let mut sample = 0;
    for (index, &chunk_offset) in chunks.iter().enumerate() {
        let chunk = index + 1;
        let per_chunk = runs
            .iter()
            .rev()
            .find(|(first, _)| *first <= chunk)
            .map_or(0, |(_, per_chunk)| *per_chunk);
        let mut offset = chunk_offset;
        for _ in 0..per_chunk {
            let Some(&size) = sizes.get(sample) else {
                break;
            };
            locations.push((offset, size));
            offset += u64::from(size);
            sample += 1;
        }
    }
    if locations.len() != count {
        return Err(invalid("sample table does not add up"));
    }
    Ok(locations)
}

/// The mean sample duration from `stts`, in the track's time scale.
fn mean_duration(stbl: &[u8], timescale: u32) -> io::Result<Duration> {
    let mut stts = Reader(child(stbl, b"stts").ok_or_else(|| invalid("no stts"))?);
    stts.take(4)?;
    let (mut samples, mut total) = (0u64, 0u64);
    for _ in 0..stts.u32()? {
        let count = u64::from(stts.u32()?);
        let delta = u64::from(stts.u32()?);
        samples += count;
        total += count * delta;
    }
    if samples == 0 || total == 0 || timescale == 0 {
        return Ok(Duration::from_micros(33_333));
    }
    Ok(Duration::from_nanos(
        (total as u128 * 1_000_000_000 / (samples as u128 * timescale as u128)) as u64,
    ))
}

impl Track {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let moov = read_moov(&file)?;
        let trak = boxes(&moov)
            .filter(|(kind, _)| kind == b"trak")
            .map(|(_, body)| body)
            .find(|trak| {
                descend(trak, &[b"mdia", b"hdlr"])
                    .and_then(|hdlr| hdlr.get(8..12))
                    .is_some_and(|handler| handler == b"vide")
            })
            .ok_or_else(|| invalid("no video track"))?;
        let mdia = child(trak, b"mdia").ok_or_else(|| invalid("no mdia"))?;
        let mut mdhd = Reader(child(mdia, b"mdhd").ok_or_else(|| invalid("no mdhd"))?);
        let version = mdhd.u8()?;
        mdhd.take(if version == 1 { 3 + 16 } else { 3 + 8 })?;
        let timescale = mdhd.u32()?;
        let stbl = descend(mdia, &[b"minf", b"stbl"]).ok_or_else(|| invalid("no stbl"))?;
        let stsd = child(stbl, b"stsd").ok_or_else(|| invalid("no stsd"))?;
        let (codec, width, height, length_size, parameter_sets) = sample_entry(stsd)?;
        let samples = sample_locations(stbl)?;
        if samples.is_empty() {
            return Err(invalid("no samples"));
        }
        let max_sample = samples
            .iter()
            .map(|(_, size)| *size as usize)
            .max()
            .unwrap_or(0);
        Ok(Self {
            file,
            codec,
            width,
            height,
            frame_interval: mean_duration(stbl, timescale)?,
            samples,
            max_sample,
            length_size,
            parameter_sets,
        })
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Writes sample `index` into `out` as Annex B, after the parameter sets
    /// when `with_sets`, using `scratch` to read it. Returns the length.
    pub fn annex_b(
        &self,
        index: usize,
        with_sets: bool,
        scratch: &mut Vec<u8>,
        out: &mut [u8],
    ) -> io::Result<usize> {
        let (offset, size) = self.samples[index];
        scratch.resize(size as usize, 0);
        self.file.read_exact_at(scratch, offset)?;
        let mut written = 0;
        let mut put = |bytes: &[u8]| -> io::Result<()> {
            let end = written + bytes.len();
            out.get_mut(written..end)
                .ok_or_else(|| invalid("sample larger than the bitstream buffer"))?
                .copy_from_slice(bytes);
            written = end;
            Ok(())
        };
        if with_sets {
            put(&self.parameter_sets)?;
        }
        let mut rest = &scratch[..];
        while rest.len() >= self.length_size {
            let length = rest[..self.length_size]
                .iter()
                .fold(0usize, |length, byte| length << 8 | *byte as usize);
            rest = &rest[self.length_size..];
            let nal = rest
                .get(..length)
                .ok_or_else(|| invalid("NAL unit runs past its sample"))?;
            put(&START_CODE)?;
            put(nal)?;
            rest = &rest[length..];
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn samples_follow_chunks() {
        // Two chunks: two samples, then one.
        let mut stbl = Vec::new();
        let mut stsz = vec![0; 4];
        stsz.extend(0u32.to_be_bytes());
        stsz.extend(3u32.to_be_bytes());
        for size in [10u32, 20, 30] {
            stsz.extend(size.to_be_bytes());
        }
        stbl.extend(mp4_box(b"stsz", &stsz));
        let mut stco = vec![0; 4];
        stco.extend(2u32.to_be_bytes());
        stco.extend(100u32.to_be_bytes());
        stco.extend(500u32.to_be_bytes());
        stbl.extend(mp4_box(b"stco", &stco));
        let mut stsc = vec![0; 4];
        stsc.extend(2u32.to_be_bytes());
        for value in [1u32, 2, 1, 2, 1, 1] {
            stsc.extend(value.to_be_bytes());
        }
        stbl.extend(mp4_box(b"stsc", &stsc));
        assert_eq!(
            sample_locations(&stbl).unwrap(),
            vec![(100, 10), (110, 20), (500, 30)]
        );
    }
}
