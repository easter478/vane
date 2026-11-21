//! Video backgrounds (MP4, AVI, WMV…) decoded with Windows Media Foundation, which ships with Windows.
//! Used for imported Lian Li L-Connect themes and for any video the user picks.

use tiny_skia::Pixmap;

pub struct Video {
    #[cfg(windows)]
    inner: Option<imp::Reader>,
    pub path: String,
    size: u32,
    started: std::time::Instant,
    offset_100ns: i64,
    frame: Option<Pixmap>,
    failed: bool,
}

impl Video {
    pub fn new(path: &str, size: u32) -> Self {
        Self {
            #[cfg(windows)]
            inner: None,
            path: path.to_string(),
            size,
            started: std::time::Instant::now(),
            offset_100ns: 0,
            frame: None,
            failed: false,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// The frame for "now" (playback speed applied), looping at the end.
    pub fn frame(&mut self, speed: f32) -> Option<&Pixmap> {
        #[cfg(windows)]
        {
            if self.failed {
                return self.frame.as_ref();
            }
            if self.inner.is_none() {
                match imp::Reader::open(&self.path) {
                    Some(r) => self.inner = Some(r),
                    None => {
                        self.failed = true;
                        return None;
                    }
                }
            }
            let want = (self.started.elapsed().as_secs_f64() * speed.clamp(0.1, 4.0) as f64 * 1e7) as i64 - self.offset_100ns;
            let r = self.inner.as_mut()?;
            let mut got = None;
            // decode forward until we reach the wanted time (at most a few frames per call)
            for _ in 0..8 {
                if r.last_ts >= want && self.frame.is_some() {
                    break;
                }
                match r.next() {
                    Some(px) => got = Some(px),
                    None => {
                        // end of the file: start again from the top
                        self.offset_100ns += r.last_ts.max(1);
                        match imp::Reader::open(&self.path) {
                            Some(n) => *r = n,
                            None => break,
                        }
                    }
                }
            }
            if let Some((w, h, bgra)) = got {
                self.frame = to_pixmap(w, h, &bgra, self.size);
            }
        }
        #[cfg(not(windows))]
        let _ = speed;
        self.frame.as_ref()
    }
}

/// BGRA frame → square RGBA pixmap of `size`, centre-cropped to fill.
fn to_pixmap(w: u32, h: u32, bgra: &[u8], size: u32) -> Option<Pixmap> {
    let mut src = Pixmap::new(w, h)?;
    for (d, s) in src.data_mut().chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = 255;
    }
    if w == size && h == size {
        return Some(src);
    }
    let mut out = Pixmap::new(size, size)?;
    let scale = (size as f32 / w as f32).max(size as f32 / h as f32);
    let tx = (size as f32 - w as f32 * scale) / 2.0;
    let ty = (size as f32 - h as f32 * scale) / 2.0;
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    out.draw_pixmap(0, 0, src.as_ref(), &paint, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty), None);
    Some(out)
}

/// First frame of a video as a JPEG thumbnail (for the background picker).
pub fn thumbnail(path: &str, size: u32) -> Option<Pixmap> {
    #[cfg(windows)]
    {
        let mut r = imp::Reader::open(path)?;
        let (w, h, bgra) = r.next()?;
        to_pixmap(w, h, &bgra, size)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, size);
        None
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    const MF_VERSION: u32 = 0x0002_0070;
    const FIRST_VIDEO: u32 = 0xFFFF_FFFC; // MF_SOURCE_READER_FIRST_VIDEO_STREAM

    pub struct Reader {
        reader: IMFSourceReader,
        w: u32,
        h: u32,
        stride: i32,
        pub last_ts: i64,
    }

    impl Reader {
        pub fn open(path: &str) -> Option<Reader> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).ok()?;
                let mut attrs: Option<IMFAttributes> = None;
                MFCreateAttributes(&mut attrs, 2).ok()?;
                let attrs = attrs?;
                // let Media Foundation convert any codec's output to plain RGB for us
                attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).ok()?;
                let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), &attrs).ok()?;
                let mt = MFCreateMediaType().ok()?;
                mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok()?;
                mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).ok()?;
                reader.SetCurrentMediaType(FIRST_VIDEO, None, &mt).ok()?;
                let cur = reader.GetCurrentMediaType(FIRST_VIDEO).ok()?;
                let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
                let (w, h) = ((fs >> 32) as u32, (fs & 0xFFFF_FFFF) as u32);
                if w == 0 || h == 0 || w > 4096 || h > 4096 {
                    return None;
                }
                let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|v| v as i32).unwrap_or((w * 4) as i32);
                Some(Reader { reader, w, h, stride, last_ts: 0 })
            }
        }

        /// Next frame as (width, height, top-down BGRA), or None at the end of the file.
        pub fn next(&mut self) -> Option<(u32, u32, Vec<u8>)> {
            unsafe {
                for _ in 0..16 {
                    let mut flags = 0u32;
                    let mut ts = 0i64;
                    let mut sample: Option<IMFSample> = None;
                    self.reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).ok()?;
                    if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                        return None;
                    }
                    let Some(sample) = sample else { continue };
                    self.last_ts = ts;
                    let buf = sample.ConvertToContiguousBuffer().ok()?;
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut len = 0u32;
                    buf.Lock(&mut ptr, None, Some(&mut len)).ok()?;
                    let row = (self.w * 4) as usize;
                    let need = row * self.h as usize;
                    let mut out = vec![0u8; need];
                    if !ptr.is_null() && len as usize >= need {
                        let src = std::slice::from_raw_parts(ptr, len as usize);
                        let stride = self.stride.unsigned_abs() as usize;
                        for y in 0..self.h as usize {
                            // a negative stride means the rows are stored bottom-up
                            let sy = if self.stride < 0 { self.h as usize - 1 - y } else { y };
                            let s0 = sy * stride.max(row);
                            if s0 + row <= src.len() {
                                out[y * row..(y + 1) * row].copy_from_slice(&src[s0..s0 + row]);
                            }
                        }
                    }
                    let _ = buf.Unlock();
                    return Some((self.w, self.h, out));
                }
                None
            }
        }
    }
}
