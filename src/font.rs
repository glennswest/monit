//! Minimal PSF (v1 and v2) bitmap font loader. We only need the raw glyph
//! bitmaps for ASCII 32..=126; the optional unicode table is ignored.

pub struct Font {
    pub width: usize,
    pub height: usize,
    bytes_per_row: usize,
    charsize: usize,
    count: usize,
    glyphs: &'static [u8],
}

fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

impl Font {
    /// Parse an embedded PSF1 or PSF2 font blob.
    pub fn parse(b: &'static [u8]) -> Font {
        if b.len() >= 4 && b[0] == 0x72 && b[1] == 0xb5 && b[2] == 0x4a && b[3] == 0x86 {
            // PSF2
            let headersize = rd_u32(b, 8) as usize;
            let count = rd_u32(b, 16) as usize;
            let charsize = rd_u32(b, 20) as usize;
            let height = rd_u32(b, 24) as usize;
            let width = rd_u32(b, 28) as usize;
            let bytes_per_row = (width + 7) / 8;
            let glyphs = &b[headersize..headersize + count * charsize];
            Font { width, height, bytes_per_row, charsize, count, glyphs }
        } else if b.len() >= 4 && b[0] == 0x36 && b[1] == 0x04 {
            // PSF1: magic(2), mode(1), charsize(1)
            let mode = b[2];
            let charsize = b[3] as usize;
            let count = if mode & 0x01 != 0 { 512 } else { 256 };
            let height = charsize;
            let width = 8;
            let bytes_per_row = 1;
            let glyphs = &b[4..4 + count * charsize];
            Font { width, height, bytes_per_row, charsize, count, glyphs }
        } else {
            panic!("unrecognized PSF font magic");
        }
    }

    /// Glyph bitmap for a character, falling back to '?' then blank.
    pub fn glyph(&self, c: char) -> &[u8] {
        let idx = c as usize;
        let idx = if idx < self.count { idx } else { '?' as usize };
        let idx = if idx < self.count { idx } else { 0 };
        &self.glyphs[idx * self.charsize..idx * self.charsize + self.charsize]
    }

    pub fn bytes_per_row(&self) -> usize {
        self.bytes_per_row
    }
}
