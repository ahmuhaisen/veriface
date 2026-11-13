//! Mutators: generic byte "havoc" (AFL style), plus structure-aware operations for JPEG marker
//! segments, PNG chunks (with CRC repair), WebP RIFF chunks, and line/token edits for YAML.

/// xorshift64*: small, fast, reproducible from a seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const INTERESTING_8: [u8; 9] = [0x00, 0x01, 0x7F, 0x80, 0xFF, 0x10, 0x20, 0x40, 0xFE];
const INTERESTING_16: [u16; 10] = [0, 1, 2, 0x7F, 0x80, 0xFF, 0x100, 0x7FFF, 0x8000, 0xFFFF];
const INTERESTING_32: [u32; 10] = [
    0,
    1,
    0x7FFF,
    0xFFFF,
    0x1_0000,
    0x3FFF,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFF,
    0xFFFF_FFFE,
];

/// Byte-level havoc. `focus` biases positions towards the first `focus` bytes (headers).
pub fn havoc(rng: &mut Rng, data: &mut Vec<u8>, corpus: &[Vec<u8>], focus: usize, max_len: usize) {
    let ops = 1 << rng.below(4);
    for _ in 0..ops {
        if data.is_empty() {
            data.push(rng.next() as u8);
            continue;
        }
        let len = data.len();
        let pos = if focus > 0 && rng.chance(0.7) {
            rng.below(len.min(focus))
        } else {
            rng.below(len)
        };
        match rng.below(14) {
            0 => data[pos] ^= 1 << rng.below(8),
            1 => data[pos] = *rng.pick(&INTERESTING_8),
            2 => data[pos] = rng.next() as u8,
            3 => data[pos] = data[pos].wrapping_add(rng.below(35) as u8).wrapping_sub(17),
            4 if pos + 2 <= len => {
                let v = *rng.pick(&INTERESTING_16);
                let b = if rng.chance(0.5) {
                    v.to_be_bytes()
                } else {
                    v.to_le_bytes()
                };
                data[pos..pos + 2].copy_from_slice(&b);
            }
            5 if pos + 4 <= len => {
                let v = *rng.pick(&INTERESTING_32);
                let b = if rng.chance(0.5) {
                    v.to_be_bytes()
                } else {
                    v.to_le_bytes()
                };
                data[pos..pos + 4].copy_from_slice(&b);
            }
            6 => {
                // Delete a block, usually small.
                let n = 1 + if rng.chance(0.9) {
                    rng.below(32)
                } else {
                    rng.below(len)
                };
                let end = (pos + n).min(len);
                data.drain(pos..end);
            }
            7 if len < max_len => {
                // Duplicate a block.
                let n = 1 + rng.below(64.min(len - pos));
                let block = data[pos..pos + n].to_vec();
                let at = rng.below(len + 1);
                data.splice(at..at, block);
            }
            8 if len < max_len => {
                let n = 1 + rng.below(32);
                let at = rng.below(len + 1);
                let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                data.splice(at..at, bytes);
            }
            9 if !corpus.is_empty() => {
                // Overwrite with bytes from another input.
                let other = rng.pick(corpus);
                if !other.is_empty() {
                    let from = rng.below(other.len());
                    let n = (1 + rng.below(64)).min(other.len() - from).min(len - pos);
                    data[pos..pos + n].copy_from_slice(&other[from..from + n]);
                }
            }
            10 if !corpus.is_empty() => {
                // Splice: our head, another input's tail.
                let other = rng.pick(corpus);
                if !other.is_empty() {
                    let from = rng.below(other.len());
                    data.truncate(pos);
                    data.extend_from_slice(&other[from..]);
                }
            }
            11 => data.truncate(pos.max(1)),
            12 => {
                // Run of a repeated byte.
                let n = (1 + rng.below(16)).min(len - pos);
                let b = *rng.pick(&INTERESTING_8);
                data[pos..pos + n].fill(b);
            }
            _ => data[pos] = !data[pos],
        }
    }
    data.truncate(max_len);
}

// ------------------------------------------------------------------------------------ JPEG

/// `(marker, offset of the marker's 0xFF, segment length incl. its 2 length bytes)` up to SOS.
fn jpeg_segments(data: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= data.len() && data[i] == 0xFF {
        let marker = data[i + 1];
        if matches!(marker, 0x01 | 0xD0..=0xD8) {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        out.push((marker, i, len));
        if marker == 0xDA || marker == 0xD9 || len < 2 {
            break;
        }
        i += 2 + len;
    }
    out
}

/// Structure-aware JPEG edits on the marker segments before the first scan.
pub fn jpeg(rng: &mut Rng, data: &mut Vec<u8>) {
    let segs = jpeg_segments(data);
    if segs.is_empty() {
        return;
    }
    let &(marker, at, len) = rng.pick(&segs);
    let body = at + 4;
    let end = (at + 2 + len).min(data.len());
    match rng.below(8) {
        0 => {
            // Length field.
            let v = *rng.pick(&INTERESTING_16);
            let v = if rng.chance(0.5) {
                v
            } else {
                (len as u16)
                    .wrapping_add(rng.below(9) as u16)
                    .wrapping_sub(4)
            };
            data[at + 2..at + 4].copy_from_slice(&v.to_be_bytes());
        }
        1 if end > at + 2 => {
            // Delete the segment.
            data.drain(at..end);
        }
        2 if end > at => {
            // Duplicate the segment.
            let seg = data[at..end].to_vec();
            data.splice(at..at, seg);
        }
        3 if marker == 0xDB && body < data.len() => {
            // DQT: precision / table id nibble, or a table value.
            if rng.chance(0.5) {
                data[body] = (rng.below(3) as u8) << 4 | rng.below(6) as u8;
            } else if body + 1 < end {
                let i = body + 1 + rng.below(end - body - 1);
                data[i] = *rng.pick(&INTERESTING_8);
            }
        }
        4 if (0xC0..=0xCF).contains(&marker)
            && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
            && body + 6 <= data.len() =>
        {
            // SOF: precision, height, width, component count, or a component's table id.
            match rng.below(5) {
                0 => data[body] = *rng.pick(&[0u8, 8, 12, 16, 255]),
                1 => data[body + 1..body + 3]
                    .copy_from_slice(&rng.pick(&INTERESTING_16).to_be_bytes()),
                2 => data[body + 3..body + 5]
                    .copy_from_slice(&rng.pick(&INTERESTING_16).to_be_bytes()),
                3 => data[body + 5] = *rng.pick(&[0u8, 1, 2, 3, 4, 5, 255]),
                _ if body + 6 < end => {
                    let i = body + 6 + rng.below(end - body - 6);
                    data[i] = rng.next() as u8;
                }
                _ => {}
            }
        }
        5 => {
            // Change the marker itself (e.g. SOF0 -> SOF2 / SOF3 / DAC).
            data[at + 1] = *rng.pick(&[
                0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC9, 0xCC, 0xDD, 0xE1, 0xFE, 0xDB, 0xC4,
            ]);
        }
        6 if body < end => {
            let i = body + rng.below(end - body);
            data[i] = rng.next() as u8;
        }
        _ => {
            // Insert a fresh DQT or DRI segment right after SOI.
            let seg: Vec<u8> = if rng.chance(0.5) {
                let pq = (rng.below(2) as u8) << 4 | rng.below(4) as u8;
                let n = if pq >> 4 == 1 { 128 } else { 64 };
                let mut s = vec![0xFF, 0xDB, 0, 0, pq];
                s.extend((0..n).map(|_| *rng.pick(&INTERESTING_8)));
                let l = (s.len() - 2) as u16;
                s[2..4].copy_from_slice(&l.to_be_bytes());
                s
            } else {
                let v = rng.pick(&INTERESTING_16).to_be_bytes();
                vec![0xFF, 0xDD, 0x00, 0x04, v[0], v[1]]
            };
            data.splice(2..2, seg);
        }
    }
}

// ------------------------------------------------------------------------------------- PNG

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// `(offset of the length field, data length)` of each chunk.
fn png_chunks(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 8;
    while i + 12 <= data.len() {
        let len = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        out.push((i, len));
        let next = i.saturating_add(12).saturating_add(len);
        if next > data.len() {
            break;
        }
        i = next;
    }
    out
}

/// Recompute every complete chunk's CRC, so mutations get past the CRC check.
pub fn png_fix_crc(data: &mut [u8]) {
    for (at, len) in png_chunks(data) {
        let end = at + 8 + len;
        if end + 4 <= data.len() {
            let crc = crc32(&data[at + 4..end]);
            data[end..end + 4].copy_from_slice(&crc.to_be_bytes());
        }
    }
}

pub fn png(rng: &mut Rng, data: &mut Vec<u8>) {
    let chunks = png_chunks(data);
    if chunks.is_empty() {
        return;
    }
    let &(at, len) = rng.pick(&chunks);
    let end = (at + 12 + len).min(data.len());
    let is_ihdr = data.get(at + 4..at + 8) == Some(b"IHDR");
    match rng.below(6) {
        0 if is_ihdr && at + 21 <= data.len() => {
            // IHDR: width, height, bit depth, colour type, compression, filter, interlace.
            let f = at + 8;
            match rng.below(5) {
                0 => data[f..f + 4].copy_from_slice(&rng.pick(&INTERESTING_32).to_be_bytes()),
                1 => data[f + 4..f + 8].copy_from_slice(&rng.pick(&INTERESTING_32).to_be_bytes()),
                2 => data[f + 8] = *rng.pick(&[1u8, 2, 4, 8, 16, 0, 3]),
                3 => data[f + 9] = *rng.pick(&[0u8, 2, 3, 4, 6, 1, 7]),
                _ => data[f + 12] = rng.below(3) as u8, // interlace (1 = Adam7)
            }
        }
        1 => data[at..at + 4].copy_from_slice(&rng.pick(&INTERESTING_32).to_be_bytes()),
        2 if end > at => {
            data.drain(at..end);
        }
        3 if end > at => {
            let chunk = data[at..end].to_vec();
            let to = rng.pick(&chunks).0;
            data.splice(to..to, chunk);
        }
        4 => {
            // Rename the chunk type (e.g. to an ancillary chunk the decoder parses).
            let names: [&[u8; 4]; 8] = [
                b"PLTE", b"tRNS", b"IDAT", b"IEND", b"acTL", b"fcTL", b"iCCP", b"eXIf",
            ];
            data[at + 4..at + 8].copy_from_slice(*rng.pick(&names));
        }
        _ if len > 0 && at + 8 + len <= data.len() => {
            let i = at + 8 + rng.below(len);
            data[i] = rng.next() as u8;
        }
        _ => {}
    }
}

// ------------------------------------------------------------------------------------ WebP

pub fn webp(rng: &mut Rng, data: &mut [u8]) {
    if data.len() <= 20 {
        return;
    }
    match rng.below(4) {
        // RIFF size, first chunk size, or a byte in the first chunk's header (VP8 frame header,
        // VP8L signature/size, VP8X canvas size).
        0 => data[4..8].copy_from_slice(&rng.pick(&INTERESTING_32).to_le_bytes()),
        1 => data[16..20].copy_from_slice(&rng.pick(&INTERESTING_32).to_le_bytes()),
        2 => {
            let i = 20 + rng.below(32.min(data.len() - 20));
            data[i] = if rng.chance(0.5) {
                rng.next() as u8
            } else {
                *rng.pick(&INTERESTING_8)
            };
        }
        _ => {
            let fourcc: [&[u8; 4]; 6] = [b"VP8 ", b"VP8L", b"VP8X", b"ANMF", b"ALPH", b"ANIM"];
            data[12..16].copy_from_slice(*rng.pick(&fourcc));
        }
    }
}

/// Make the RIFF size consistent with the file length (half of the time a mutation should keep
/// the outer container valid to reach the inner parsers).
pub fn webp_fix_riff(data: &mut [u8]) {
    if data.len() >= 12 && &data[0..4] == b"RIFF" {
        let size = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&size.to_le_bytes());
    }
}

// ------------------------------------------------------------------------------------ YAML

const YAML_VALUES: &[&str] = &[
    "0",
    "-1",
    "1",
    "1e308",
    "-1e308",
    "1e-320",
    ".nan",
    ".NaN",
    ".inf",
    "-.inf",
    "-0",
    "18446744073709551616",
    "4294967296",
    "4294967295",
    "-9223372036854775809",
    "0x10",
    "0o17",
    "1_000",
    "~",
    "null",
    "[]",
    "{}",
    "\"\"",
    "''",
    "true",
    "no",
    "yes",
    "\"x\"",
    "&a 1",
    "*a",
    "!!binary AAAA",
    "!!str 5",
    "[1, [2, [3]]]",
    "{a: {b: {c: 1}}}",
    "|\n  multi",
    "?",
    "- -",
    "\"\\u0000\"",
    "\"\\uD800\"",
    "jpeg",
    "png",
    "webp",
    "gif",
    "error",
    "warning",
    "standard",
    "FACE_PRESENT",
    "HEAD_POSE_YAW",
    "provisional",
    "calibrated",
    "portrait-3x4",
];

fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_owned).collect()
}

/// Line and token edits on YAML text; occasionally byte havoc (lossily re-decoded as UTF-8).
pub fn yaml(rng: &mut Rng, text: &mut String, corpus: &[Vec<u8>]) {
    let ops = 1 + rng.below(4);
    for _ in 0..ops {
        let mut lines = split_lines(text);
        if lines.is_empty() {
            lines.push(String::new());
        }
        let i = rng.below(lines.len());
        match rng.below(12) {
            0 if lines.len() > 1 => {
                lines.remove(i);
            }
            1 => {
                let l = lines[i].clone();
                lines.insert(i, l);
            }
            2 => {
                let j = rng.below(lines.len());
                lines.swap(i, j);
            }
            3 | 4 => {
                // Replace the value after the first ':' (or a flow-mapping value) with another.
                let line = &lines[i];
                if let Some(c) = line.find(": ") {
                    let rest = &line[c + 2..];
                    let cut = if rest.starts_with('{') {
                        rest.find([',', '}']).unwrap_or(0)
                    } else {
                        rest.len()
                    };
                    let v = *rng.pick(YAML_VALUES);
                    lines[i] = format!("{}: {}{}", &line[..c], v, &rest[cut..]);
                }
            }
            5 => {
                // Replace a number anywhere in the line.
                let line = lines[i].clone();
                let digits: Vec<usize> = line
                    .char_indices()
                    .filter(|(_, c)| c.is_ascii_digit())
                    .map(|(k, _)| k)
                    .collect();
                if !digits.is_empty() {
                    let s = *rng.pick(&digits);
                    let e = line[s..]
                        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'e' || c == '-'))
                        .map_or(line.len(), |k| s + k);
                    lines[i] = format!("{}{}{}", &line[..s], rng.pick(YAML_VALUES), &line[e..]);
                }
            }
            6 => {
                // Re-indent.
                if rng.chance(0.5) {
                    lines[i].insert_str(0, "  ");
                } else if lines[i].starts_with("  ") {
                    lines[i].drain(..2);
                } else {
                    lines[i].insert(0, '\t');
                }
            }
            7 => {
                // Copy a key from another line (duplicate keys, keys in the wrong section).
                let j = rng.below(lines.len());
                if let (Some(a), Some(b)) = (lines[i].find(':'), lines[j].find(':')) {
                    let key = lines[j][..b].trim_start().to_owned();
                    let indent = lines[i].len() - lines[i].trim_start().len();
                    lines[i] = format!("{}{}{}", " ".repeat(indent), key, &lines[i][a..]);
                }
            }
            8 => {
                // Nesting bombs and alias tricks.
                let depth = 1 + rng.below(4000);
                let bomb = match rng.below(4) {
                    0 => format!("x: {}{}", "[".repeat(depth), "]".repeat(depth)),
                    1 => format!("x: {}1{}", "{a: ".repeat(depth), "}".repeat(depth)),
                    2 => (0..depth.min(60))
                        .map(|k| format!("{}- ", "  ".repeat(k)))
                        .collect::<String>(),
                    _ => {
                        // Alias expansion ("billion laughs").
                        let mut s = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
                        let levels = 2 + rng.below(9);
                        for k in 1..levels {
                            let refs = vec![format!("*a{}", k - 1); 10].join(", ");
                            s.push_str(&format!("a{k}: &a{k} [{refs}]\n"));
                        }
                        s
                    }
                };
                lines.insert(i, bomb);
            }
            9 if !corpus.is_empty() => {
                // Splice lines from another input.
                let other = String::from_utf8_lossy(rng.pick(corpus)).into_owned();
                let other = split_lines(&other);
                let from = rng.below(other.len());
                let n = 1 + rng.below(8);
                for (k, l) in other.iter().skip(from).take(n).enumerate() {
                    lines.insert((i + k).min(lines.len()), l.clone());
                }
            }
            10 => {
                let mut bytes = text.clone().into_bytes();
                havoc(rng, &mut bytes, corpus, 0, 1 << 20);
                *text = String::from_utf8_lossy(&bytes).into_owned();
                continue;
            }
            _ => {
                // Insert a structural character.
                let ch = *rng.pick(&[
                    ":", "-", "?", "&", "*", "!", "|", ">", "#", "%", "@", "`", "\"", "'", "{",
                    "[", ",", "\u{FEFF}", "\r",
                ]);
                let line = &mut lines[i];
                let at = (0..=line.len())
                    .filter(|k| line.is_char_boundary(*k))
                    .nth(rng.below(line.chars().count() + 1))
                    .unwrap_or(0);
                line.insert_str(at, ch);
            }
        }
        *text = lines.join("\n");
    }
    let mut cap = text.len().min(1 << 20);
    while !text.is_char_boundary(cap) {
        cap -= 1;
    }
    text.truncate(cap);
}
