//! MP4 fragmentado: cada trecho de cerca de 2 s já fica válido no disco,
//! então uma queda de energia não perde a gravação inteira.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Codec {
    Av1,
    Hevc,
    H264,
}

#[derive(Clone, Debug)]
pub struct Pkt {
    pub data: Vec<u8>,
    /// Tempo em unidades de 100 ns no relógio do sistema.
    pub pts: i64,
    pub key: bool,
}

const VIDEO_SCALE: u64 = 90000;
const AUDIO_SCALE: u64 = 48000;
const AAC_CONFIG: [u8; 2] = [0x11, 0x90];
const FRAGMENT: i64 = 20_000_000;

fn bx(name: &[u8; 4], f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut b = vec![0, 0, 0, 0];
    b.extend_from_slice(name);
    f(&mut b);
    let n = b.len() as u32;
    b[..4].copy_from_slice(&n.to_be_bytes());
    b
}

fn full(name: &[u8; 4], version: u8, flags: u32, f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    bx(name, |b| {
        b.push(version);
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        f(b);
    })
}

fn u16b(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_be_bytes());
}
fn u32b(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_be_bytes());
}
fn u64b(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_be_bytes());
}

const MATRIX: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000];

/// Lê bits do cabeçalho de sequência do AV1.
struct Bits<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bits<'_> {
    fn get(&mut self, n: usize) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            let byte = self.d.get(self.p / 8).copied().unwrap_or(0);
            v = v << 1 | ((byte >> (7 - self.p % 8)) & 1) as u32;
            self.p += 1;
        }
        v
    }
}

/// Separa as OBUs e devolve (tipo, faixa completa, início do conteúdo).
fn obus(d: &[u8]) -> Vec<(u8, std::ops::Range<usize>, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let h = d[i];
        let ty = (h >> 3) & 15;
        let ext = (h >> 2) & 1;
        let has_size = (h >> 1) & 1;
        let mut j = i + 1 + ext as usize;
        let mut size = d.len().saturating_sub(j);
        if has_size == 1 {
            size = 0;
            let mut shift = 0;
            while j < d.len() {
                let b = d[j];
                j += 1;
                size |= ((b & 0x7f) as usize) << shift;
                shift += 7;
                if b & 0x80 == 0 {
                    break;
                }
            }
        }
        let end = (j + size).min(d.len());
        out.push((ty, i..end, j));
        i = end;
    }
    out
}

fn av1c(key: &[u8], ten_bit: bool) -> Vec<u8> {
    let list = obus(key);
    let seq = list.iter().find(|o| o.0 == 1);
    let (mut profile, mut level, mut tier) = (0u8, 8u8, 0u8);
    let mut config = Vec::new();
    if let Some((_, range, start)) = seq {
        config = key[range.clone()].to_vec();
        let mut b = Bits { d: &key[*start..range.end], p: 0 };
        profile = b.get(3) as u8;
        b.get(1);
        let reduced = b.get(1);
        if reduced == 1 {
            level = b.get(5) as u8;
        } else {
            if b.get(1) == 1 {
                b.get(32);
                b.get(32);
                if b.get(1) == 1 {
                    let mut z = 0;
                    while b.get(1) == 0 && z < 32 {
                        z += 1;
                    }
                    b.get(z);
                }
                b.get(1);
            }
            b.get(1);
            b.get(5);
            b.get(12);
            level = b.get(5) as u8;
            if level > 7 {
                tier = b.get(1) as u8;
            }
        }
    }
    bx(b"av1C", |b| {
        b.push(0x81);
        b.push(profile << 5 | level);
        b.push(tier << 7 | (ten_bit as u8) << 6 | 1 << 3 | 1 << 2);
        b.push(0);
        b.extend_from_slice(&config);
    })
}

fn nals(d: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= d.len() {
        if d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, s) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(d.len(), |e| e - 3);
        while end > *s && d[end - 1] == 0 {
            end -= 1;
        }
        out.push(&d[*s..end]);
    }
    out
}

fn avcc(key: &[u8]) -> Vec<u8> {
    let n = nals(key);
    let sps = n.iter().find(|x| !x.is_empty() && x[0] & 31 == 7).copied().unwrap_or(&[0, 100, 0, 51]);
    let pps = n.iter().find(|x| !x.is_empty() && x[0] & 31 == 8).copied().unwrap_or(&[]);
    bx(b"avcC", |b| {
        b.extend_from_slice(&[1, sps[1], sps[2], sps[3], 0xff, 0xe1]);
        u16b(b, sps.len() as u16);
        b.extend_from_slice(sps);
        b.push(1);
        u16b(b, pps.len() as u16);
        b.extend_from_slice(pps);
    })
}

fn hvcc(key: &[u8]) -> Vec<u8> {
    let n = nals(key);
    let kind = |t: u8| n.iter().find(|x| !x.is_empty() && (x[0] >> 1) & 63 == t).copied();
    let vps = kind(32).unwrap_or(&[]);
    let sps = kind(33).unwrap_or(&[]);
    let pps = kind(34).unwrap_or(&[]);
    // Sem os bytes de prevenção, o perfil começa logo após o cabeçalho do SPS.
    let mut clean = Vec::new();
    let mut z = 0;
    for &b in sps.iter().skip(2) {
        if z >= 2 && b == 3 {
            z = 0;
            continue;
        }
        z = if b == 0 { z + 1 } else { 0 };
        clean.push(b);
    }
    clean.resize(clean.len().max(13), 0);
    bx(b"hvcC", |b| {
        b.push(1);
        b.extend_from_slice(&clean[1..13]);
        b.extend_from_slice(&[0xf0, 0, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 0x0f, 3]);
        for (t, nal) in [(32u8, vps), (33, sps), (34, pps)] {
            b.push(0x80 | t);
            u16b(b, 1);
            u16b(b, nal.len() as u16);
            b.extend_from_slice(nal);
        }
    })
}

/// Tira o que o MP4 não aceita dentro das amostras.
fn sample_bytes(codec: Codec, d: &[u8]) -> Vec<u8> {
    match codec {
        Codec::Av1 => {
            let mut out = Vec::with_capacity(d.len());
            for (ty, r, _) in obus(d) {
                if ty != 2 {
                    out.extend_from_slice(&d[r]);
                }
            }
            out
        }
        _ => {
            let mut out = Vec::with_capacity(d.len() + 16);
            for n in nals(d) {
                out.extend_from_slice(&(n.len() as u32).to_be_bytes());
                out.extend_from_slice(n);
            }
            out
        }
    }
}

pub struct Mp4 {
    f: BufWriter<File>,
    codec: Codec,
    w: u32,
    h: u32,
    ten_bit: bool,
    fps: u32,
    audio: bool,
    started: bool,
    t0: i64,
    seq: u32,
    v: Vec<Pkt>,
    a: Vec<Pkt>,
    a_dts: u64,
    a_pending: i64,
    last_pts: i64,
    mehd: u64,
    index: Vec<(u64, u64)>,
    pub bytes: u64,
}

impl Mp4 {
    pub fn create(path: &str, codec: Codec, w: u32, h: u32, fps: u32, ten_bit: bool, audio: bool) -> std::io::Result<Self> {
        Ok(Self {
            f: BufWriter::with_capacity(1 << 20, File::create(path)?),
            codec,
            w,
            h,
            ten_bit,
            fps,
            audio,
            started: false,
            t0: 0,
            seq: 0,
            v: Vec::new(),
            a: Vec::new(),
            a_dts: 0,
            a_pending: 0,
            last_pts: 0,
            mehd: 0,
            index: Vec::new(),
            bytes: 0,
        })
    }

    fn entry(&self, key: &[u8]) -> Vec<u8> {
        let (name, config): (&[u8; 4], Vec<u8>) = match self.codec {
            Codec::Av1 => (b"av01", av1c(key, self.ten_bit)),
            Codec::Hevc => (b"hvc1", hvcc(key)),
            Codec::H264 => (b"avc1", avcc(key)),
        };
        let (prim, trc, mat): (u16, u16, u16) = if self.ten_bit { (9, 16, 9) } else { (1, 1, 1) };
        bx(name, |b| {
            b.extend_from_slice(&[0; 6]);
            u16b(b, 1);
            b.extend_from_slice(&[0; 16]);
            u16b(b, self.w as u16);
            u16b(b, self.h as u16);
            u32b(b, 0x480000);
            u32b(b, 0x480000);
            u32b(b, 0);
            u16b(b, 1);
            b.extend_from_slice(&[0; 32]);
            u16b(b, 0x18);
            u16b(b, 0xffff);
            b.extend_from_slice(&config);
            b.extend_from_slice(&bx(b"colr", |c| {
                c.extend_from_slice(b"nclx");
                u16b(c, prim);
                u16b(c, trc);
                u16b(c, mat);
                c.push(0);
            }));
        })
    }

    fn trak(&self, id: u32, entry: Vec<u8>, audio: bool) -> Vec<u8> {
        let scale = if audio { AUDIO_SCALE } else { VIDEO_SCALE } as u32;
        bx(b"trak", |t| {
            t.extend_from_slice(&full(b"tkhd", 0, 3, |b| {
                u32b(b, 0);
                u32b(b, 0);
                u32b(b, id);
                u32b(b, 0);
                u32b(b, 0);
                u64b(b, 0);
                u16b(b, 0);
                u16b(b, 0);
                u16b(b, if audio { 0x100 } else { 0 });
                u16b(b, 0);
                for m in MATRIX {
                    u32b(b, m);
                }
                u32b(b, if audio { 0 } else { self.w << 16 });
                u32b(b, if audio { 0 } else { self.h << 16 });
            }));
            t.extend_from_slice(&bx(b"mdia", |m| {
                m.extend_from_slice(&full(b"mdhd", 0, 0, |b| {
                    u32b(b, 0);
                    u32b(b, 0);
                    u32b(b, scale);
                    u32b(b, 0);
                    u16b(b, 0x55c4);
                    u16b(b, 0);
                }));
                m.extend_from_slice(&full(b"hdlr", 0, 0, |b| {
                    u32b(b, 0);
                    b.extend_from_slice(if audio { b"soun" } else { b"vide" });
                    b.extend_from_slice(&[0; 12]);
                    b.extend_from_slice(if audio { b"Audio\0" } else { b"Video\0" });
                }));
                m.extend_from_slice(&bx(b"minf", |i| {
                    if audio {
                        i.extend_from_slice(&full(b"smhd", 0, 0, |b| u32b(b, 0)));
                    } else {
                        i.extend_from_slice(&full(b"vmhd", 0, 1, |b| u64b(b, 0)));
                    }
                    i.extend_from_slice(&bx(b"dinf", |d| {
                        d.extend_from_slice(&full(b"dref", 0, 0, |b| {
                            u32b(b, 1);
                            b.extend_from_slice(&full(b"url ", 0, 1, |_| {}));
                        }));
                    }));
                    i.extend_from_slice(&bx(b"stbl", |s| {
                        s.extend_from_slice(&full(b"stsd", 0, 0, |b| {
                            u32b(b, 1);
                            b.extend_from_slice(&entry);
                        }));
                        s.extend_from_slice(&full(b"stts", 0, 0, |b| u32b(b, 0)));
                        s.extend_from_slice(&full(b"stsc", 0, 0, |b| u32b(b, 0)));
                        s.extend_from_slice(&full(b"stsz", 0, 0, |b| u64b(b, 0)));
                        s.extend_from_slice(&full(b"stco", 0, 0, |b| u32b(b, 0)));
                    }));
                }));
            }));
        })
    }

    fn header(&mut self, key: &[u8]) -> std::io::Result<()> {
        let ftyp = bx(b"ftyp", |b| {
            b.extend_from_slice(b"iso6");
            u32b(b, 0);
            for c in [b"iso6", b"isom", b"mp41", b"av01", b"dash"] {
                b.extend_from_slice(c);
            }
        });
        let mut mehd_rel = 0;
        let tracks = if self.audio { 2 } else { 1 };
        let moov = bx(b"moov", |m| {
            m.extend_from_slice(&full(b"mvhd", 0, 0, |b| {
                u32b(b, 0);
                u32b(b, 0);
                u32b(b, 1000);
                u32b(b, 0);
                u32b(b, 0x10000);
                u16b(b, 0x100);
                b.extend_from_slice(&[0; 10]);
                for v in MATRIX {
                    u32b(b, v);
                }
                b.extend_from_slice(&[0; 24]);
                u32b(b, tracks + 1);
            }));
            m.extend_from_slice(&self.trak(1, self.entry(key), false));
            if self.audio {
                let esds = full(b"esds", 0, 0, |b| {
                    b.extend_from_slice(&[0x03, 25, 0, 2, 0]);
                    b.extend_from_slice(&[0x04, 17, 0x40, 0x15, 0, 0, 0]);
                    u32b(b, 192000);
                    u32b(b, 192000);
                    b.extend_from_slice(&[0x05, 2]);
                    b.extend_from_slice(&AAC_CONFIG);
                    b.extend_from_slice(&[0x06, 1, 2]);
                });
                let mp4a = bx(b"mp4a", |b| {
                    b.extend_from_slice(&[0; 6]);
                    u16b(b, 1);
                    b.extend_from_slice(&[0; 8]);
                    u16b(b, 2);
                    u16b(b, 16);
                    u32b(b, 0);
                    u32b(b, (AUDIO_SCALE as u32) << 16);
                    b.extend_from_slice(&esds);
                });
                m.extend_from_slice(&self.trak(2, mp4a, true));
            }
            mehd_rel = m.len() + 8 + 12;
            m.extend_from_slice(&bx(b"mvex", |x| {
                x.extend_from_slice(&full(b"mehd", 1, 0, |b| u64b(b, 0)));
                for id in 1..=tracks {
                    x.extend_from_slice(&full(b"trex", 0, 0, |b| {
                        u32b(b, id);
                        u32b(b, 1);
                        u32b(b, 0);
                        u32b(b, 0);
                        u32b(b, 0);
                    }));
                }
            }));
        });
        self.f.write_all(&ftyp)?;
        self.f.write_all(&moov)?;
        self.bytes = (ftyp.len() + moov.len()) as u64;
        self.mehd = ftyp.len() as u64 + mehd_rel as u64;
        Ok(())
    }

    pub fn push_video(&mut self, p: &Pkt) -> std::io::Result<()> {
        if !self.started {
            if !p.key {
                return Ok(());
            }
            self.header(&p.data)?;
            self.started = true;
            self.t0 = p.pts;
            self.a.retain(|a| a.pts >= p.pts);
        }
        if p.key && p.pts - self.v.first().map_or(p.pts, |f| f.pts) >= FRAGMENT {
            self.flush(p.pts)?;
        }
        self.last_pts = p.pts;
        self.v.push(Pkt { data: sample_bytes(self.codec, &p.data), pts: p.pts, key: p.key });
        Ok(())
    }

    pub fn push_audio(&mut self, p: &Pkt) {
        if self.audio && (!self.started || p.pts >= self.t0) {
            self.a.push(p.clone());
        }
    }

    fn flush(&mut self, end: i64) -> std::io::Result<()> {
        if self.v.is_empty() {
            return Ok(());
        }
        let v = std::mem::take(&mut self.v);
        let split = self.a.iter().position(|a| a.pts >= end).unwrap_or(self.a.len());
        let a: Vec<Pkt> = self.a.drain(..split).collect();
        self.seq += 1;
        let conv = |t: i64| (t.max(0) as u64) * VIDEO_SCALE / 10_000_000;
        let base = conv(v[0].pts - self.t0);

        let mut vdata_off_at = 0;
        let mut adata_off_at = 0;
        let moof = bx(b"moof", |m| {
            m.extend_from_slice(&full(b"mfhd", 0, 0, |b| u32b(b, self.seq)));
            let start = m.len();
            let traf = bx(b"traf", |t| {
                t.extend_from_slice(&full(b"tfhd", 0, 0x020000, |b| u32b(b, 1)));
                t.extend_from_slice(&full(b"tfdt", 1, 0, |b| u64b(b, base)));
                let trun = full(b"trun", 0, 0x701, |b| {
                    u32b(b, v.len() as u32);
                    u32b(b, 0);
                    for (i, s) in v.iter().enumerate() {
                        let next = v.get(i + 1).map_or(end, |n| n.pts);
                        let dur = conv(next - self.t0) - conv(s.pts - self.t0);
                        u32b(b, dur.max(1) as u32);
                        u32b(b, s.data.len() as u32);
                        u32b(b, if s.key { 0x0200_0000 } else { 0x0101_0000 });
                    }
                });
                vdata_off_at = start + t.len() + 16;
                t.extend_from_slice(&trun);
            });
            m.extend_from_slice(&traf);
            if !a.is_empty() {
                let start = m.len();
                let traf = bx(b"traf", |t| {
                    t.extend_from_slice(&full(b"tfhd", 0, 0x020000, |b| u32b(b, 2)));
                    t.extend_from_slice(&full(b"tfdt", 1, 0, |b| u64b(b, self.a_dts)));
                    let trun = full(b"trun", 0, 0x301, |b| {
                        u32b(b, a.len() as u32);
                        u32b(b, 0);
                        for s in &a {
                            u32b(b, 1024);
                            u32b(b, s.data.len() as u32);
                        }
                    });
                    adata_off_at = start + t.len() + 16;
                    t.extend_from_slice(&trun);
                });
                m.extend_from_slice(&traf);
            }
        });
        let mut moof = moof;
        let vsize: usize = v.iter().map(|s| s.data.len()).sum();
        let asize: usize = a.iter().map(|s| s.data.len()).sum();
        let voff = (moof.len() + 8) as u32;
        moof[vdata_off_at..vdata_off_at + 4].copy_from_slice(&voff.to_be_bytes());
        if !a.is_empty() {
            let aoff = voff + vsize as u32;
            moof[adata_off_at..adata_off_at + 4].copy_from_slice(&aoff.to_be_bytes());
            self.a_dts += a.len() as u64 * 1024;
        }
        self.index.push((base, self.bytes));
        self.f.write_all(&moof)?;
        self.f.write_all(&((vsize + asize + 8) as u32).to_be_bytes())?;
        self.f.write_all(b"mdat")?;
        for s in &v {
            self.f.write_all(&s.data)?;
        }
        for s in &a {
            self.f.write_all(&s.data)?;
        }
        self.bytes += (moof.len() + 8 + vsize + asize) as u64;
        self.f.flush()
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        if !self.started {
            return Ok(());
        }
        let step = 10_000_000 / self.fps.max(1) as i64;
        let end = self.last_pts + step;
        self.a_pending = end;
        self.flush(end)?;
        let tfra = full(b"tfra", 1, 0, |b| {
            u32b(b, 1);
            u32b(b, 0);
            u32b(b, self.index.len() as u32);
            for (t, off) in &self.index {
                u64b(b, *t);
                u64b(b, *off);
                b.extend_from_slice(&[1, 1, 1]);
            }
        });
        let total = tfra.len() as u32 + 8 + 16;
        let mfra = bx(b"mfra", |m| {
            m.extend_from_slice(&tfra);
            m.extend_from_slice(&full(b"mfro", 0, 0, |b| u32b(b, total)));
        });
        self.f.write_all(&mfra)?;
        let ms = ((end - self.t0).max(0) / 10_000) as u64;
        self.f.seek(SeekFrom::Start(self.mehd))?;
        self.f.write_all(&ms.to_be_bytes())?;
        self.f.flush()
    }
}
