// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Multi-threaded PNG encoder for cairo ARGB32 pixels.
//!
//! cairo's `write_to_png` (libpng) filters and deflates on one core, which
//! takes ~0.3 s for a 5120×1440 screenshot and over a second for a few 4K
//! monitors. Here the rows are split into strips that are filtered (adaptive,
//! like libpng) and deflated (zlib) in parallel; the strips are joined
//! into one zlib stream with sync flushes. Output is a standard 8-bit RGB PNG
//! (RGBA when the image has transparency).

/// Encodes premultiplied native-endian ARGB32 pixels (cairo's layout).
pub fn encode(width: usize, height: usize, stride: usize, data: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > i32::MAX as usize / 4 || stride < width * 4 || data.len() < stride * (height - 1) + width * 4 {
        return Err("invalid image size".into());
    }
    let opaque = is_opaque(width, height, stride, data);
    let bpp = if opaque { 3 } else { 4 };
    let row_len = 1 + width * bpp;

    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8);
    // strips of at least 32 rows; a few more strips than threads balances uneven content
    let n_strips = (height / 32).clamp(1, threads * 2);
    let per = height.div_ceil(n_strips);
    let ranges: Vec<(usize, usize)> = (0..height).step_by(per).map(|y| (y, (y + per).min(height))).collect();
    let last = ranges.len() - 1;

    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<(Vec<u8>, u32, usize), String>>>> =
        ranges.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..threads.min(ranges.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&(y0, y1)) = ranges.get(i) else { break };
                let raw = filter_rows(width, stride, data, bpp, y0, y1);
                let adler = adler32(&raw);
                let r = deflate(&raw, i == last).map(|z| (z, adler, raw.len()));
                *results[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            });
        }
    });

    let mut idat = Vec::with_capacity(height * row_len / 4 + 64);
    idat.extend_from_slice(&[0x78, 0x9c]); // zlib header: deflate, 32K window
    let mut adler = 1u32;
    for r in results {
        let (z, a, len) = r.into_inner().unwrap_or_else(|e| e.into_inner()).ok_or("encoder thread failed")??;
        idat.extend_from_slice(&z);
        adler = adler32_combine(adler, a, len);
    }
    idat.extend_from_slice(&adler.to_be_bytes());

    let mut out = Vec::with_capacity(idat.len() + 128);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if opaque { 2 } else { 6 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    // IDAT chunks of at most 1 MiB, as most encoders do
    for part in idat.chunks(1 << 20) {
        chunk(&mut out, b"IDAT", part);
    }
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn is_opaque(width: usize, height: usize, stride: usize, data: &[u8]) -> bool {
    (0..height).all(|y| {
        data[y * stride..y * stride + width * 4]
            .chunks_exact(4)
            .all(|p| u32::from_ne_bytes([p[0], p[1], p[2], p[3]]) >> 24 == 0xff)
    })
}

/// One image row as straight (non-premultiplied) RGB or RGBA bytes.
fn convert_row(src: &[u8], bpp: usize, out: &mut [u8]) {
    for (p, o) in src.chunks_exact(4).zip(out.chunks_exact_mut(bpp)) {
        let v = u32::from_ne_bytes([p[0], p[1], p[2], p[3]]);
        let (a, r, g, b) = (v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
        if bpp == 3 {
            o[0] = r as u8;
            o[1] = g as u8;
            o[2] = b as u8;
        } else {
            let un = |c: u32| if a == 0 { 0 } else { ((c * 255 + a / 2) / a).min(255) as u8 };
            if a == 255 {
                o.copy_from_slice(&[r as u8, g as u8, b as u8, 255]);
            } else {
                o.copy_from_slice(&[un(r), un(g), un(b), a as u8]);
            }
        }
    }
}

/// Filtered scanlines (filter byte + data) for rows y0..y1.
fn filter_rows(width: usize, stride: usize, data: &[u8], bpp: usize, y0: usize, y1: usize) -> Vec<u8> {
    let n = width * bpp;
    let mut prev = vec![0u8; n];
    let mut cur = vec![0u8; n];
    if y0 > 0 {
        convert_row(&data[(y0 - 1) * stride..(y0 - 1) * stride + width * 4], bpp, &mut prev);
    }
    let mut out = Vec::with_capacity((y1 - y0) * (n + 1));
    let mut cand: [Vec<u8>; 5] = std::array::from_fn(|_| vec![0u8; n]);
    for y in y0..y1 {
        convert_row(&data[y * stride..y * stride + width * 4], bpp, &mut cur);
        let f = filter_row(&cur, &prev, bpp, &mut cand);
        out.push(f as u8);
        out.extend_from_slice(&cand[f]);
        std::mem::swap(&mut prev, &mut cur);
    }
    out
}

/// libpng's heuristic: the filter whose output has the smallest sum of
/// absolute (signed) byte values. Returns the index into `cand`.
/// One pass computes all four predictors and their scores; the first `bpp`
/// bytes (no left neighbour) are handled separately so the main loop has no
/// branches or bounds checks.
fn filter_row(cur: &[u8], prev: &[u8], bpp: usize, cand: &mut [Vec<u8>; 5]) -> usize {
    let n = cur.len();
    let [none, sub, up, avg, paeth] = cand;
    none.copy_from_slice(cur);
    let abs = |v: u8| (v as i8).unsigned_abs() as u32;
    let mut score = [0u64; 5];
    score[0] = cur.iter().map(|&v| abs(v) as u64).sum();
    let head = bpp.min(n);
    for i in 0..head {
        let (x, b) = (cur[i], prev[i]);
        sub[i] = x;
        up[i] = x.wrapping_sub(b);
        avg[i] = x.wrapping_sub(b / 2);
        paeth[i] = x.wrapping_sub(b); // paeth with a = c = 0 predicts b
    }
    let mut acc = [0u32; 4];
    for k in 0..head {
        acc[0] += abs(sub[k]);
        acc[1] += abs(up[k]);
        acc[2] += abs(avg[k]);
        acc[3] += abs(paeth[k]);
    }
    // per-row sums fit in u32 (at most 128 per byte)
    let it = cur[head..]
        .iter()
        .zip(&cur[..n - head])
        .zip(prev[head..].iter().zip(&prev[..n - head]))
        .zip(sub[head..].iter_mut().zip(up[head..].iter_mut()))
        .zip(avg[head..].iter_mut().zip(paeth[head..].iter_mut()));
    for ((((&x, &a), (&b, &c)), (s, u)), (v, p)) in it {
        *s = x.wrapping_sub(a);
        *u = x.wrapping_sub(b);
        *v = x.wrapping_sub(((a as u16 + b as u16) >> 1) as u8);
        *p = x.wrapping_sub(paeth_pred(a, b, c));
        acc[0] += abs(*s);
        acc[1] += abs(*u);
        acc[2] += abs(*v);
        acc[3] += abs(*p);
    }
    for k in 0..4 {
        score[k + 1] = acc[k] as u64;
    }
    let mut best = 0;
    for i in 1..5 {
        if score[i] < score[best] {
            best = i;
        }
    }
    best
}

#[inline(always)]
fn paeth_pred(a: u8, b: u8, c: u8) -> u8 {
    let (ai, bi, ci) = (a as i16, b as i16, c as i16);
    let pa = (bi - ci).abs();
    let pb = (ai - ci).abs();
    let pc = (ai + bi - 2 * ci).abs();
    let ab = if pb < pa { b } else { a };
    if pc < pa.min(pb) {
        c
    } else {
        ab
    }
}

/// Raw deflate of one strip. Strips other than the last end with a sync
/// flush (byte aligned, not final) so they can simply be concatenated.
/// Uses the system zlib (already loaded by GLib / libpng) with the settings
/// libpng uses for filtered images: level 6, `Z_FILTERED`.
fn deflate(input: &[u8], last: bool) -> Result<Vec<u8>, String> {
    use std::os::raw::{c_char, c_int, c_uint, c_ulong, c_void};
    #[repr(C)]
    struct ZStream {
        next_in: *const u8,
        avail_in: c_uint,
        total_in: c_ulong,
        next_out: *mut u8,
        avail_out: c_uint,
        total_out: c_ulong,
        msg: *const c_char,
        state: *mut c_void,
        zalloc: *const c_void,
        zfree: *const c_void,
        opaque: *mut c_void,
        data_type: c_int,
        adler: c_ulong,
        reserved: c_ulong,
    }
    #[link(name = "z")]
    unsafe extern "C" {
        fn zlibVersion() -> *const c_char;
        fn deflateInit2_(s: *mut ZStream, level: c_int, method: c_int, bits: c_int, mem: c_int, strategy: c_int, v: *const c_char, size: c_int) -> c_int;
        fn deflate(s: *mut ZStream, flush: c_int) -> c_int;
        fn deflateBound(s: *mut ZStream, len: c_ulong) -> c_ulong;
        fn deflateEnd(s: *mut ZStream) -> c_int;
    }
    const Z_SYNC_FLUSH: c_int = 2;
    const Z_FINISH: c_int = 4;
    const Z_OK: c_int = 0;
    const Z_STREAM_END: c_int = 1;
    const Z_BUF_ERROR: c_int = -5;

    // SAFETY: plain zlib usage; the stream is initialised by deflateInit2_,
    // in/out pointers always cover live buffers of the stated lengths, and
    // deflateEnd runs on every path after a successful init.
    unsafe {
        let mut zs: ZStream = std::mem::zeroed();
        let size = std::mem::size_of::<ZStream>() as c_int;
        if deflateInit2_(&mut zs, 6, 8, -15, 8, 1, zlibVersion(), size) != Z_OK {
            return Err("zlib init failed".into());
        }
        let mut out: Vec<u8> = Vec::with_capacity(deflateBound(&mut zs, input.len() as c_ulong) as usize + 64);
        let mut pos = 0;
        let result = loop {
            let n = (input.len() - pos).min(1 << 30);
            zs.next_in = input.as_ptr().add(pos);
            zs.avail_in = n as c_uint;
            if out.capacity() - out.len() < 64 {
                out.reserve(out.capacity() / 2 + 1024);
            }
            let room = (out.capacity() - out.len()).min(1 << 30);
            zs.next_out = out.as_mut_ptr().add(out.len());
            zs.avail_out = room as c_uint;
            let flush = if pos + n < input.len() { 0 } else if last { Z_FINISH } else { Z_SYNC_FLUSH };
            let r = deflate(&mut zs, flush);
            pos += n - zs.avail_in as usize;
            out.set_len(out.len() + room - zs.avail_out as usize);
            match r {
                Z_STREAM_END => break Ok(()),
                // a sync flush is complete once all input is used and output space is left
                Z_OK if flush == Z_SYNC_FLUSH && zs.avail_out > 0 => break Ok(()),
                Z_OK | Z_BUF_ERROR => {}
                e => break Err(format!("zlib error {e}")),
            }
        };
        deflateEnd(&mut zs);
        result.map(|_| out)
    }
}

const ADLER_MOD: u32 = 65521;

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the largest block for which the sums cannot overflow u32
    for block in data.chunks(5552) {
        for &x in block {
            a += x as u32;
            b += a;
        }
        a %= ADLER_MOD;
        b %= ADLER_MOD;
    }
    (b << 16) | a
}

/// adler32 of the concatenation, from the checksums of both parts (zlib's adler32_combine).
fn adler32_combine(a1: u32, a2: u32, len2: usize) -> u32 {
    let m = ADLER_MOD as u64;
    let rem = len2 as u64 % m;
    let mut sum1 = (a1 & 0xffff) as u64;
    let mut sum2 = (rem * sum1) % m;
    sum1 += (a2 & 0xffff) as u64 + m - 1;
    sum2 += ((a1 >> 16) & 0xffff) as u64 + ((a2 >> 16) & 0xffff) as u64 + m - rem;
    sum1 %= m;
    sum2 %= m;
    (sum1 | (sum2 << 16)) as u32
}

fn crc32(parts: &[&[u8]]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, e) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = !0u32;
    for p in parts {
        for &b in *p {
            c = t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(png: &[u8]) -> cairo::ImageSurface {
        cairo::ImageSurface::create_from_png(&mut std::io::Cursor::new(png)).expect("valid png")
    }

    fn pixels(s: &cairo::ImageSurface) -> Vec<u32> {
        s.flush();
        let (w, h, st) = (s.width() as usize, s.height() as usize, s.stride() as usize);
        let mut v = Vec::new();
        s.with_data(|d| {
            for y in 0..h {
                for x in 0..w {
                    let o = y * st + x * 4;
                    v.push(u32::from_ne_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]));
                }
            }
        })
        .unwrap();
        v
    }

    fn image(w: usize, h: usize, alpha: impl Fn(usize, usize) -> u32) -> (Vec<u8>, usize) {
        let stride = w * 4 + 8; // padded rows
        let mut d = vec![0u8; stride * h];
        for y in 0..h {
            for x in 0..w {
                let a = alpha(x, y);
                let c = |v: usize| ((v as u32 & 0xff) * a / 255) & 0xff;
                let px = a << 24 | c(x * 7 + y) << 16 | c(y * 3) << 8 | c(x ^ y);
                d[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&px.to_ne_bytes());
            }
        }
        (d, stride)
    }

    #[test]
    fn opaque_roundtrip() {
        for (w, h) in [(1, 1), (3, 2), (257, 300), (640, 33)] {
            let (d, stride) = image(w, h, |_, _| 255);
            let png = encode(w, h, stride, &d).unwrap();
            assert_eq!(png[25], 2, "RGB colour type for opaque images");
            let s = decode(&png);
            assert_eq!((s.width() as usize, s.height() as usize), (w, h));
            let (orig, _) = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).fold((Vec::new(), ()), |(mut v, _), (x, y)| {
                let o = y * stride + x * 4;
                v.push(u32::from_ne_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]));
                (v, ())
            });
            assert_eq!(pixels(&s), orig);
        }
    }

    #[test]
    fn alpha_roundtrip() {
        let (w, h) = (100, 90);
        let (d, stride) = image(w, h, |x, y| if x < 10 { 0 } else if y < 30 { 128 } else { 255 });
        let png = encode(w, h, stride, &d).unwrap();
        assert_eq!(png[25], 6, "RGBA colour type with transparency");
        let s = decode(&png);
        let got = pixels(&s);
        for y in 0..h {
            for x in 0..w {
                let o = y * stride + x * 4;
                let want = u32::from_ne_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
                let g = got[y * w + x];
                assert_eq!(g >> 24, want >> 24);
                for sh in [0, 8, 16] {
                    let (a, b) = ((g >> sh) & 0xff, (want >> sh) & 0xff);
                    assert!(a.abs_diff(b) <= 1, "pixel {x},{y}: {g:08x} vs {want:08x}");
                }
            }
        }
    }

    #[test]
    fn adler_combine_matches() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i * 31 % 251) as u8).collect();
        for split in [0, 1, 5552, 77_777, 100_000] {
            let (a, b) = data.split_at(split);
            assert_eq!(adler32_combine(adler32(a), adler32(b), b.len()), adler32(&data));
        }
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
    }

    #[test]
    fn paeth_matches_spec() {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                for c in (0..=255u8).step_by(3) {
                    let p = a as i16 + b as i16 - c as i16;
                    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
                    let want = if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c };
                    assert_eq!(paeth_pred(a, b, c), want, "{a} {b} {c}");
                }
            }
        }
    }

    #[test]
    fn rejects_bad_sizes() {
        assert!(encode(0, 1, 4, &[0; 4]).is_err());
        assert!(encode(2, 2, 8, &[0; 8]).is_err());
    }
}

