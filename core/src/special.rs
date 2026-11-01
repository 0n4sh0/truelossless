//! Header-only identification of formats Symphonia cannot decode (Dolby
//! E-AC-3/Atmos, AC-3, DTS, TrueHD, WavPack, Monkey's Audio, DSD, ...).
//!
//! These files are not analysed spectrally. We only name the codec and say
//! whether it is lossy or lossless, so the user gets an honest answer
//! instead of a false "corrupt".

use crate::probe::Container;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Largest MP4 `moov` box we will read while looking for the codec.
const MAX_MOOV: u64 = 16 * 1024 * 1024;
/// How far into a Matroska file we search for the codec ID.
const MKV_SCAN: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind {
    Lossy,
    Lossless,
    Unknown,
}

/// What the header says about a file we cannot decode.
#[derive(Debug, Clone, PartialEq)]
pub struct Special {
    pub container: Container,
    pub codec: String,
    pub kind: CodecKind,
    pub sample_rate: Option<u32>,
    pub channels: Option<usize>,
    pub bits: Option<u32>,
    pub duration_secs: Option<f64>,
    /// Extra detail, e.g. "Dolby Atmos (JOC)" or "5.1".
    pub note: Option<String>,
}

impl Special {
    fn new(container: Container, codec: &str, kind: CodecKind) -> Self {
        Special {
            container,
            codec: codec.into(),
            kind,
            sample_rate: None,
            channels: None,
            bits: None,
            duration_secs: None,
            note: None,
        }
    }
}

/// Identify `path` from its header. `None` means "not one of these formats".
pub fn identify(path: &Path) -> std::io::Result<Option<Special>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut head = vec![0u8; 4096.min(len as usize)];
    f.read_exact(&mut head)?;

    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return mp4_special(&mut f, len);
    }
    if head.len() >= 4 && head[0] == 0x1A && head[1] == 0x45 && head[2] == 0xDF && head[3] == 0xA3 {
        let mut big = head.clone();
        big.resize(MKV_SCAN.min(len as usize), 0);
        f.seek(SeekFrom::Start(0))?;
        f.read_exact(&mut big)?;
        return Ok(Some(matroska(&big)));
    }
    Ok(identify_bytes(&head))
}

/// Identification from the first bytes of a raw or simple container.
pub fn identify_bytes(b: &[u8]) -> Option<Special> {
    let starts = |m: &[u8]| b.len() >= m.len() && &b[..m.len()] == m;

    if starts(&[0x0B, 0x77]) && b.len() >= 6 {
        // bsid > 10 marks Enhanced AC-3.
        let bsid = b[5] >> 3;
        let (name, container) = if bsid > 10 {
            ("E-AC-3", Container::Ac3)
        } else {
            ("AC-3", Container::Ac3)
        };
        return Some(Special::new(container, name, CodecKind::Lossy));
    }
    if starts(&[0x7F, 0xFE, 0x80, 0x01]) {
        let mut s = Special::new(Container::Dts, "DTS", CodecKind::Lossy);
        s.note = Some("core stream; DTS-HD extensions not inspected".into());
        return Some(s);
    }
    if b.len() >= 8 && b[4..7] == [0xF8, 0x72, 0x6F] && (b[7] == 0xBA || b[7] == 0xBB) {
        let name = if b[7] == 0xBA { "TrueHD" } else { "MLP" };
        return Some(Special::new(Container::Mlp, name, CodecKind::Lossless));
    }
    if starts(b"wvpk") && b.len() >= 28 {
        let flags = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
        let hybrid = flags & 0x8 != 0;
        let mut s = Special::new(
            Container::WavPack,
            "WavPack",
            if hybrid {
                CodecKind::Lossy
            } else {
                CodecKind::Lossless
            },
        );
        if hybrid {
            s.note = Some("hybrid mode".into());
        }
        return Some(s);
    }
    if starts(b"MAC ") {
        return Some(Special::new(
            Container::Ape,
            "Monkey's Audio",
            CodecKind::Lossless,
        ));
    }
    if starts(b"TTA1") {
        return Some(Special::new(Container::Tta, "TTA", CodecKind::Lossless));
    }
    if starts(b"DSD ") {
        return Some(dsf(b));
    }
    if starts(b"FRM8") && b.len() >= 16 && &b[12..16] == b"DSD " {
        return Some(Special::new(
            Container::Dsd,
            "DSD (DFF)",
            CodecKind::Lossless,
        ));
    }
    if starts(b"MPCK") || starts(b"MP+") {
        return Some(Special::new(
            Container::Musepack,
            "Musepack",
            CodecKind::Lossy,
        ));
    }
    if starts(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]) {
        let mut s = Special::new(Container::Asf, "WMA", CodecKind::Unknown);
        s.note = Some("lossy or lossless not determined".into());
        return Some(s);
    }
    if starts(b"OggS") && contains(&b[..b.len().min(512)], b"OpusHead") {
        return Some(Special::new(Container::Ogg, "Opus", CodecKind::Lossy));
    }
    None
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// DSF header: sample rate and channel count sit in the `fmt ` chunk.
fn dsf(b: &[u8]) -> Special {
    let mut s = Special::new(Container::Dsd, "DSD (DSF)", CodecKind::Lossless);
    if b.len() >= 60 && &b[28..32] == b"fmt " {
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        s.channels = Some(u32_at(52) as usize);
        s.sample_rate = Some(u32_at(56));
        s.bits = Some(1);
        if b.len() >= 72 && s.sample_rate.is_some_and(|r| r > 0) {
            let frames = u64::from_le_bytes(b[64..72].try_into().unwrap());
            s.duration_secs = Some(frames as f64 / s.sample_rate.unwrap() as f64);
        }
    }
    s
}

fn matroska(b: &[u8]) -> Special {
    const IDS: &[(&[u8], &str, CodecKind)] = &[
        (b"A_TRUEHD", "TrueHD", CodecKind::Lossless),
        (b"A_MLP", "MLP", CodecKind::Lossless),
        (b"A_EAC3", "E-AC-3", CodecKind::Lossy),
        (b"A_AC3", "AC-3", CodecKind::Lossy),
        (
            b"A_DTS/LOSSLESS",
            "DTS-HD Master Audio",
            CodecKind::Lossless,
        ),
        (b"A_DTS", "DTS", CodecKind::Lossy),
        (b"A_OPUS", "Opus", CodecKind::Lossy),
        (b"A_WAVPACK4", "WavPack", CodecKind::Lossless),
        (b"A_TTA1", "TTA", CodecKind::Lossless),
        (b"A_MS/ACM", "ACM", CodecKind::Unknown),
    ];
    for (id, name, kind) in IDS {
        if contains(b, id) {
            return Special::new(Container::Matroska, name, *kind);
        }
    }
    // Codecs Symphonia would handle in other containers.
    let mut s = Special::new(Container::Matroska, "Matroska audio", CodecKind::Unknown);
    s.note = Some("codec not recognised".into());
    s
}

// ---------------------------------------------------------------------------
// MP4 sample entries
// ---------------------------------------------------------------------------

/// Walk the top-level boxes for `moov`, then look for a codec sample entry.
fn mp4_special(f: &mut File, len: u64) -> std::io::Result<Option<Special>> {
    let mut pos = 0u64;
    while pos + 8 <= len {
        let mut h = [0u8; 16];
        f.seek(SeekFrom::Start(pos))?;
        let n = f.read(&mut h)?;
        if n < 8 {
            break;
        }
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let mut header = 8;
        if size == 1 && n >= 16 {
            size = u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]);
            header = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < header {
            break;
        }
        if &h[4..8] == b"moov" {
            let body = (size - header).min(MAX_MOOV);
            let mut buf = vec![0u8; body as usize];
            f.seek(SeekFrom::Start(pos + header))?;
            f.read_exact(&mut buf)?;
            let mut sp = mp4_sample_entry(&buf);
            if let Some(s) = sp.as_mut() {
                s.duration_secs = mp4_duration(&buf);
                if s.duration_secs.is_none() {
                    s.duration_secs = fragment_duration(f, pos + size, len, &buf, s.sample_rate)?;
                }
            }
            return Ok(sp);
        }
        pos = pos.saturating_add(size);
    }
    Ok(None)
}

/// Duration from `mvhd`, or from `mehd` for fragmented files where `mvhd`
/// stores 0.
pub fn mp4_duration(moov: &[u8]) -> Option<f64> {
    let at = moov.windows(4).position(|w| w == b"mvhd")? + 4;
    let p = moov.get(at..)?;
    let (timescale, dur) = match *p.first()? {
        0 => (
            u32::from_be_bytes(p.get(12..16)?.try_into().ok()?),
            u32::from_be_bytes(p.get(16..20)?.try_into().ok()?) as u64,
        ),
        1 => (
            u32::from_be_bytes(p.get(20..24)?.try_into().ok()?),
            u64::from_be_bytes(p.get(24..32)?.try_into().ok()?),
        ),
        _ => return None,
    };
    if timescale == 0 {
        return None;
    }
    let mut dur = dur;
    if dur == 0 {
        if let Some(m) = moov.windows(4).position(|w| w == b"mehd") {
            let q = moov.get(m + 4..)?;
            dur = match *q.first()? {
                0 => u32::from_be_bytes(q.get(4..8)?.try_into().ok()?) as u64,
                _ => u64::from_be_bytes(q.get(4..12)?.try_into().ok()?),
            };
        }
    }
    (dur > 0).then(|| dur as f64 / timescale as f64)
}

/// Duration of a fragmented file with no total stored: add up the sample
/// counts in every `moof/trun`, times the default sample duration in `trex`.
fn fragment_duration(
    f: &mut File,
    mut pos: u64,
    len: u64,
    moov: &[u8],
    media_rate: Option<u32>,
) -> std::io::Result<Option<f64>> {
    let Some(rate) = media_rate else {
        return Ok(None);
    };
    // trex: version/flags(4) track_id(4) desc_index(4) default_sample_duration(4)
    let trex_unit = moov
        .windows(4)
        .position(|w| w == b"trex")
        .and_then(|t| moov.get(t + 16..t + 20))
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64)
        .unwrap_or(0);
    let mut ticks = 0u64;
    let mut unit = trex_unit;
    while pos + 8 <= len {
        let mut h = [0u8; 8];
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut h)?;
        let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        if size < 8 {
            break;
        }
        if &h[4..8] == b"moof" && size <= MAX_MOOV {
            let mut body = vec![0u8; (size - 8) as usize];
            f.read_exact(&mut body)?;
            // A fragment without its own default reuses the previous one.
            unit = tfhd_duration(&body).unwrap_or(unit);
            if unit == 0 {
                return Ok(None);
            }
            let mut from = 0;
            while let Some(i) = body[from..].windows(4).position(|w| w == b"trun") {
                let at = from + i + 4;
                let Some(b) = body.get(at..at + 8) else { break };
                let flags = u32::from_be_bytes([0, b[1], b[2], b[3]]);
                let count = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as usize;
                if flags & 0x100 == 0 {
                    ticks += count as u64 * unit;
                } else {
                    // Per-sample table: walk it and add the stored durations.
                    let mut o = at + 8;
                    if flags & 0x1 != 0 {
                        o += 4;
                    }
                    if flags & 0x4 != 0 {
                        o += 4;
                    }
                    let stride = 4 * (flags & 0xF00).count_ones() as usize;
                    for i in 0..count {
                        let Some(d) = body.get(o + i * stride..o + i * stride + 4) else {
                            return Ok(None);
                        };
                        ticks += u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as u64;
                    }
                }
                from = at + 8;
            }
        }
        pos += size;
    }
    Ok((ticks > 0).then(|| ticks as f64 / rate as f64))
}

/// `default_sample_duration` from a `tfhd` box, when present.
fn tfhd_duration(moof: &[u8]) -> Option<u64> {
    let at = moof.windows(4).position(|w| w == b"tfhd")? + 4;
    let p = moof.get(at..)?;
    let flags = u32::from_be_bytes([0, *p.get(1)?, *p.get(2)?, *p.get(3)?]);
    if flags & 0x8 == 0 {
        return None;
    }
    let mut o = 8; // version/flags + track_id
    if flags & 0x1 != 0 {
        o += 8;
    }
    if flags & 0x2 != 0 {
        o += 4;
    }
    let b = p.get(o..o + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64).filter(|&u| u > 0)
}

/// Find the audio sample entry inside `moov`. Only codecs Symphonia cannot
/// decode are returned.
pub fn mp4_sample_entry(moov: &[u8]) -> Option<Special> {
    type Entry = (&'static [u8; 4], &'static str, CodecKind);
    const ENTRIES: &[Entry] = &[
        (b"ec-3", "E-AC-3", CodecKind::Lossy),
        (b"ac-3", "AC-3", CodecKind::Lossy),
        (b"ac-4", "AC-4", CodecKind::Lossy),
        (b"mhm1", "MPEG-H 3D Audio", CodecKind::Lossy),
        (b"mha1", "MPEG-H 3D Audio", CodecKind::Lossy),
        (b"mlpa", "TrueHD", CodecKind::Lossless),
        (b"dtsl", "DTS-HD Master Audio", CodecKind::Lossless),
        (b"dtsh", "DTS-HD", CodecKind::Lossy),
        (b"dtsc", "DTS", CodecKind::Lossy),
        (b"dtse", "DTS Express", CodecKind::Lossy),
        (b"Opus", "Opus", CodecKind::Lossy),
    ];
    for (tag, name, kind) in ENTRIES {
        let Some(at) = moov.windows(4).position(|w| w == *tag) else {
            continue;
        };
        let mut s = Special::new(Container::Mp4, name, *kind);
        // AudioSampleEntry: 4 tag + 6 reserved + 2 index + 8 reserved, then
        // channelcount(2) samplesize(2) pre_defined(2) reserved(2) rate(16.16).
        let e = &moov[at..];
        if e.len() >= 32 {
            let ch = u16::from_be_bytes([e[20], e[21]]) as usize;
            let rate = u32::from_be_bytes([e[28], e[29], e[30], e[31]]) >> 16;
            if ch > 0 {
                s.channels = Some(ch);
            }
            if rate > 0 {
                s.sample_rate = Some(rate);
            }
        }
        if &tag[..] == b"ec-3" {
            if let Some(d) = dec3(moov) {
                s.note = Some(d.describe());
                if d.channels > 0 {
                    s.channels = Some(d.channels);
                }
            }
        } else if &tag[..] == b"ac-4" {
            s.note = Some("may carry Dolby Atmos".into());
        }
        return Some(s);
    }
    None
}

/// Fields of the `dec3` (EC3SpecificBox) box that matter to us.
#[derive(Debug, PartialEq)]
pub struct Dec3 {
    pub data_rate_kbps: u32,
    /// Channels of the independent substream (bed), LFE included.
    pub channels: usize,
    pub joc: bool,
    pub complexity: u8,
}

impl Dec3 {
    pub fn describe(&self) -> String {
        let mut s = if self.joc {
            format!("Dolby Atmos (JOC, complexity {})", self.complexity)
        } else {
            "Dolby Digital Plus".to_string()
        };
        if self.data_rate_kbps > 0 {
            s.push_str(&format!(", {} kbps", self.data_rate_kbps));
        }
        s
    }
}

const ACMOD_CHANNELS: [usize; 8] = [2, 1, 2, 3, 3, 4, 4, 5];

/// Parse the first `dec3` box found in `data`.
pub fn dec3(data: &[u8]) -> Option<Dec3> {
    let at = data.windows(4).position(|w| w == b"dec3")? + 4;
    let p = &data[at..];
    if p.len() < 5 {
        return None;
    }
    let mut r = Bits { b: p, pos: 0 };
    let data_rate = r.take(13)?;
    let num_ind = r.take(3)? + 1;
    let mut channels = 0;
    for i in 0..num_ind {
        r.take(2)?; // fscod
        r.take(5)?; // bsid
        r.take(1)?; // reserved
        r.take(1)?; // asvc
        r.take(3)?; // bsmod
        let acmod = r.take(3)? as usize;
        let lfe = r.take(1)? as usize;
        r.take(3)?; // reserved
        let num_dep = r.take(4)?;
        if num_dep > 0 {
            r.take(9)?; // chan_loc
        } else {
            r.take(1)?; // reserved
        }
        if i == 0 {
            channels = ACMOD_CHANNELS[acmod] + lfe;
        }
    }
    // Optional extension: 7 reserved bits, type-A flag, complexity index.
    let (mut joc, mut complexity) = (false, 0);
    if r.remaining() >= 16 {
        r.take(7)?;
        joc = r.take(1)? == 1;
        complexity = r.take(8)? as u8;
    }
    Some(Dec3 {
        data_rate_kbps: data_rate,
        channels,
        joc,
        complexity,
    })
}

struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn remaining(&self) -> usize {
        self.b.len() * 8 - self.pos
    }
    fn take(&mut self, n: usize) -> Option<u32> {
        if self.remaining() < n {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            let bit = (self.b[self.pos / 8] >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_raw_magics() {
        let ac3 = identify_bytes(&[0x0B, 0x77, 0, 0, 0, 8 << 3]).unwrap();
        assert_eq!(ac3.codec, "AC-3");
        let eac3 = identify_bytes(&[0x0B, 0x77, 0, 0, 0, 16 << 3]).unwrap();
        assert_eq!(eac3.codec, "E-AC-3");
        assert_eq!(eac3.kind, CodecKind::Lossy);
        assert_eq!(
            identify_bytes(&[0x7F, 0xFE, 0x80, 0x01]).unwrap().codec,
            "DTS"
        );
        let thd = identify_bytes(&[0, 0, 0, 0, 0xF8, 0x72, 0x6F, 0xBA]).unwrap();
        assert_eq!(
            (thd.codec.as_str(), thd.kind),
            ("TrueHD", CodecKind::Lossless)
        );
        assert_eq!(
            identify_bytes(b"MAC \x96\x0f").unwrap().codec,
            "Monkey's Audio"
        );
        assert_eq!(identify_bytes(b"TTA1").unwrap().kind, CodecKind::Lossless);
        assert!(identify_bytes(b"fLaC\0\0\0\x22").is_none());
        assert!(identify_bytes(b"RIFF\x24\0\0\0WAVE").is_none());
    }

    #[test]
    fn wavpack_hybrid_is_lossy() {
        let mut h = vec![0u8; 32];
        h[..4].copy_from_slice(b"wvpk");
        assert_eq!(identify_bytes(&h).unwrap().kind, CodecKind::Lossless);
        h[24] = 0x08;
        assert_eq!(identify_bytes(&h).unwrap().kind, CodecKind::Lossy);
    }

    #[test]
    fn reads_dsf_header() {
        let mut h = vec![0u8; 64];
        h[..4].copy_from_slice(b"DSD ");
        h[28..32].copy_from_slice(b"fmt ");
        h[52..56].copy_from_slice(&2u32.to_le_bytes());
        h[56..60].copy_from_slice(&2_822_400u32.to_le_bytes());
        let s = identify_bytes(&h).unwrap();
        assert_eq!((s.channels, s.sample_rate), (Some(2), Some(2_822_400)));
    }

    #[test]
    fn opus_in_ogg() {
        let mut h = b"OggS\0\x02".to_vec();
        h.extend_from_slice(&[0; 22]);
        h.extend_from_slice(b"OpusHead\x01\x02");
        assert_eq!(identify_bytes(&h).unwrap().codec, "Opus");
    }

    #[test]
    fn matroska_codec_ids() {
        let mut d = vec![0x1A, 0x45, 0xDF, 0xA3];
        d.extend_from_slice(b"....A_TRUEHD....");
        assert_eq!(matroska(&d).codec, "TrueHD");
    }

    /// `dec3` payload: 640 kbps, 1 substream, 5.1 bed, JOC complexity 16.
    fn dec3_box(joc: bool) -> Vec<u8> {
        let mut bits = String::new();
        bits += &format!("{:013b}", 640); // data_rate
        bits += "000"; // num_ind_sub - 1
        bits += "00"; // fscod
        bits += "10000"; // bsid
        bits += "0"; // reserved
        bits += "0"; // asvc
        bits += "000"; // bsmod
        bits += "111"; // acmod 3/2
        bits += "1"; // lfe
        bits += "000"; // reserved
        bits += "0000"; // num_dep_sub
        bits += "0"; // reserved
        if joc {
            bits += "0000000";
            bits += "1";
            bits += &format!("{:08b}", 16);
        }
        while !bits.len().is_multiple_of(8) {
            bits.push('0');
        }
        let mut out = b"dec3".to_vec();
        for c in bits.as_bytes().chunks(8) {
            out.push(u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap());
        }
        out
    }

    #[test]
    fn parses_atmos_dec3() {
        let d = dec3(&dec3_box(true)).unwrap();
        assert_eq!(
            d,
            Dec3 {
                data_rate_kbps: 640,
                channels: 6,
                joc: true,
                complexity: 16
            }
        );
        assert!(d.describe().contains("Atmos"));
        let plain = dec3(&dec3_box(false)).unwrap();
        assert!(!plain.joc);
        assert!(plain.describe().contains("Digital Plus"));
    }

    #[test]
    fn reads_mvhd_and_mehd_duration() {
        // v0 mvhd: ver/flags, ctime, mtime, timescale 1000, duration 183500.
        let mut m = b"mvhd    ".to_vec();
        m.extend_from_slice(&[0; 8]);
        m.extend_from_slice(&1000u32.to_be_bytes());
        m.extend_from_slice(&183_500u32.to_be_bytes());
        assert_eq!(mp4_duration(&m), Some(183.5));
        // Fragmented: mvhd duration 0, mehd carries it.
        let mut f = b"mvhd    ".to_vec();
        f.extend_from_slice(&[0; 8]);
        f.extend_from_slice(&1000u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(b"mehd    ");
        f.extend_from_slice(&90_000u32.to_be_bytes());
        assert_eq!(mp4_duration(&f), Some(90.0));
    }

    #[test]
    fn mp4_sample_entry_reports_atmos() {
        // Minimal moov payload: an ec-3 sample entry followed by dec3.
        let mut m = vec![0u8; 8];
        m.extend_from_slice(b"ec-3");
        m.extend_from_slice(&[0; 6]);
        m.extend_from_slice(&[0, 1]);
        m.extend_from_slice(&[0; 8]);
        m.extend_from_slice(&[0, 6, 0, 16, 0, 0, 0, 0]);
        m.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
        m.extend_from_slice(&dec3_box(true));
        let s = mp4_sample_entry(&m).unwrap();
        assert_eq!(s.codec, "E-AC-3");
        assert_eq!(s.sample_rate, Some(48_000));
        assert_eq!(s.channels, Some(6));
        assert!(s.note.unwrap().contains("Atmos"));
    }
}
