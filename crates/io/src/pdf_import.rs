//! Opening PDFs (File › Open, Place), as Photoshop does.
//!
//! A PDF that PhotoCraft saved with its editing data (Photoshop's "Preserve Photoshop Editing
//! Capabilities") carries the layered document; it opens as that document, layers and all, and
//! saving writes the PDF again. Any other PDF is rasterised a page at a time (Photoshop's Import
//! PDF): the page as a transparent "Layer 1" at a chosen resolution, cropped to one of the page's
//! boxes. Pages are rendered in pure Rust by `hayro` (text, vector art, images, shadings).
//!
//! The editing data is a `.pcraft` bundle in a stream that the first page's page-piece dictionary
//! names (`/PieceInfo /PhotoCraft /Private /Document`, ISO 32000-1 § 14.5), where PDF lets an
//! application keep its own data; other readers ignore it.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt as _;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Dict, Stream};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{Point, Rect};
use hayro::vello_cpu::color::palette::css::{TRANSPARENT, WHITE};
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use photocraft_codecs::{ChannelLayout, Image};
use photocraft_doc::Document;
use photocraft_raster::Rgba8Image;

use crate::{ImportResult, IoError};

/// The page-piece key PhotoCraft's private data is filed under.
pub const PIECE: &[u8] = b"PhotoCraft";

/// The largest page side that renders (the renderer's limit); a page past it needs a lower
/// resolution.
pub const MAX_SIDE: u32 = 30_000;

/// Pixels a rendered page may have (about 1.2 GB of RGBA8 while it renders).
const MAX_PIXELS: u64 = 300_000_000;

/// `true` if `bytes` look like a PDF (the header may follow up to 1 KB of junk, as readers allow).
pub fn is_pdf(bytes: &[u8]) -> bool {
    bytes.get(..bytes.len().min(1024)).is_some_and(|head| head.windows(5).any(|w| w == b"%PDF-"))
}

/// A page's size in points, as shown (crop box, rotation applied).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfPage {
    pub width_pt: f64,
    pub height_pt: f64,
}

/// What Import PDF shows before anything is rendered.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfInfo {
    pub pages: Vec<PdfPage>,
    /// PhotoCraft saved it with its editing data: it opens as the layered document.
    pub editable: bool,
}

/// Which of the page's boxes the image covers (Photoshop's Crop To).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PdfCrop {
    /// The smallest rectangle around everything the page paints.
    BoundingBox,
    MediaBox,
    /// The page as viewers show it.
    #[default]
    CropBox,
    BleedBox,
    TrimBox,
    ArtBox,
}

impl PdfCrop {
    pub const ALL: [PdfCrop; 6] = [PdfCrop::BoundingBox, PdfCrop::MediaBox, PdfCrop::CropBox, PdfCrop::BleedBox, PdfCrop::TrimBox, PdfCrop::ArtBox];

    pub fn label(self) -> &'static str {
        match self {
            PdfCrop::BoundingBox => "Bounding Box",
            PdfCrop::MediaBox => "Media Box",
            PdfCrop::CropBox => "Crop Box",
            PdfCrop::BleedBox => "Bleed Box",
            PdfCrop::TrimBox => "Trim Box",
            PdfCrop::ArtBox => "Art Box",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            PdfCrop::BoundingBox => "boundingBox",
            PdfCrop::MediaBox => "mediaBox",
            PdfCrop::CropBox => "cropBox",
            PdfCrop::BleedBox => "bleedBox",
            PdfCrop::TrimBox => "trimBox",
            PdfCrop::ArtBox => "artBox",
        }
    }

    pub fn from_id(id: &str) -> Option<PdfCrop> {
        PdfCrop::ALL.into_iter().find(|c| c.id().eq_ignore_ascii_case(id))
    }

    /// The page dictionary key of the box (`None` for the bounding box).
    fn key(self) -> Option<&'static [u8]> {
        match self {
            PdfCrop::BoundingBox => None,
            PdfCrop::MediaBox => Some(b"MediaBox"),
            PdfCrop::CropBox => Some(b"CropBox"),
            PdfCrop::BleedBox => Some(b"BleedBox"),
            PdfCrop::TrimBox => Some(b"TrimBox"),
            PdfCrop::ArtBox => Some(b"ArtBox"),
        }
    }
}

/// How a page is rasterised.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfRaster {
    /// Pixels per inch (Photoshop's default is 300).
    pub ppi: f32,
    pub crop: PdfCrop,
}

impl Default for PdfRaster {
    fn default() -> Self {
        PdfRaster { ppi: 300.0, crop: PdfCrop::CropBox }
    }
}

fn load(bytes: &[u8]) -> Result<Pdf, IoError> {
    use hayro::hayro_syntax::LoadPdfError;
    Pdf::new(bytes.to_vec()).map_err(|e| match e {
        LoadPdfError::Decryption(d) => IoError::Unsupported(format!("this PDF is encrypted ({d:?}); open it in a PDF reader and save an unprotected copy")),
        LoadPdfError::Invalid => IoError::Unsupported("not a readable PDF".into()),
    })
}

fn page<'a>(pdf: &'a Pdf, index: usize) -> Result<&'a Page<'a>, IoError> {
    let pages = pdf.pages();
    pages.get(index).ok_or_else(|| IoError::Unsupported(format!("the PDF has {} page(s); there is no page {}", pages.len(), index + 1)))
}

/// The page count, the page sizes, and whether PhotoCraft's editing data is inside.
pub fn info(bytes: &[u8]) -> Result<PdfInfo, IoError> {
    let pdf = load(bytes)?;
    let pages: Vec<PdfPage> = pdf
        .pages()
        .iter()
        .map(|p| {
            let (w, h) = p.render_dimensions();
            PdfPage { width_pt: f64::from(w), height_pt: f64::from(h) }
        })
        .collect();
    if pages.is_empty() {
        return Err(IoError::Unsupported("the PDF has no pages".into()));
    }
    Ok(PdfInfo { pages, editable: editing_stream(&pdf).is_some() })
}

/// The first page's PhotoCraft editing data stream.
fn editing_stream<'a>(pdf: &'a Pdf) -> Option<Stream<'a>> {
    let first = pdf.pages().first()?;
    let piece = first.raw().get::<Dict<'_>>(b"PieceInfo")?.get::<Dict<'_>>(PIECE)?;
    piece.get::<Dict<'_>>(b"Private")?.get::<Stream<'_>>(b"Document")
}

/// The layered document PhotoCraft saved in the PDF, if it is there.
pub fn editable_document(bytes: &[u8]) -> Option<Result<Document, IoError>> {
    let pdf = load(bytes).ok()?;
    let stream = editing_stream(&pdf)?;
    Some(match stream.decoded() {
        Ok(data) => photocraft_format::load_from_bytes(&data).map_err(IoError::from),
        Err(_) => Err(IoError::Unsupported("the PDF's PhotoCraft editing data is damaged".into())),
    })
}

/// Fonts: the PDF's own, else the standard 14 and CJK fallbacks hayro carries.
fn interpreter() -> InterpreterSettings {
    InterpreterSettings::default()
}

/// The page rendered at `scale` pixels per point over `bg`, as straight-alpha RGBA8.
fn render(page: &Page<'_>, scale: f64, bg: hayro::vello_cpu::color::AlphaColor<hayro::vello_cpu::color::Srgb>) -> Result<(u32, u32, Vec<u8>), IoError> {
    let (w, h) = page.render_dimensions();
    let (pw, ph) = ((f64::from(w) * scale).round(), (f64::from(h) * scale).round());
    if !(pw.is_finite() && ph.is_finite()) || pw < 1.0 || ph < 1.0 {
        return Err(IoError::Unsupported("the page is too small to render at this resolution".into()));
    }
    if pw > f64::from(MAX_SIDE) || ph > f64::from(MAX_SIDE) || pw * ph > MAX_PIXELS as f64 {
        return Err(IoError::Unsupported(format!("the page would be {pw} × {ph} px; choose a lower resolution")));
    }
    // Scales that land on the rounded size (the renderer truncates): 166.7 px is 167, as in
    // Photoshop.
    let exact = |px: f64, pt: f32| {
        let mut k = (px / f64::from(pt)) as f32;
        // The renderer sizes the pixmap as `(pt * k) as u16`, in f32.
        while ((pt * k) as u16) < px as u16 {
            k = k.next_up();
        }
        k
    };
    let settings = PixmapSettings { x_scale: exact(pw, w), y_scale: exact(ph, h), bg_color: bg };
    let cache = RenderCache::new();
    let pixmap = hayro::render(page, &cache, &interpreter(), &RenderSettings::default(), &settings);
    let (rw, rh) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    Ok((rw, rh, pixmap.take_rgba8(ImageAlphaType::Alpha)))
}

/// The pixel rectangle `(x0, y0, x1, y1)` of `crop` in a render of `page` at `scale`, clamped to
/// the render (boxes the page doesn't have fall back to the crop box, as in Photoshop).
fn crop_pixels(page: &Page<'_>, crop: PdfCrop, scale: f64, (w, h): (u32, u32)) -> (u32, u32, u32, u32) {
    let full = (0, 0, w, h);
    let Some(key) = crop.key() else { return full };
    let Some(b) = page.raw().get::<hayro::hayro_syntax::object::Rect>(key) else { return full };
    let t = page.initial_transform(true).to_kurbo();
    let r = Rect::new(b.x0.min(b.x1), b.y0.min(b.y1), b.x0.max(b.x1), b.y0.max(b.y1));
    let corners = [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x0, r.y1), Point::new(r.x1, r.y1)].map(|p| t * p);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in corners {
        x0 = x0.min(p.x * scale);
        y0 = y0.min(p.y * scale);
        x1 = x1.max(p.x * scale);
        y1 = y1.max(p.y * scale);
    }
    let clamp = |v: f64, max: u32| if v.is_finite() { v.round().clamp(0.0, f64::from(max)) as u32 } else { 0 };
    let (x0, y0, x1, y1) = (clamp(x0, w), clamp(y0, h), clamp(x1, w), clamp(y1, h));
    if x1 <= x0 || y1 <= y0 { full } else { (x0, y0, x1, y1) }
}

/// The smallest rectangle holding every painted (not fully transparent) pixel.
fn painted_bounds(w: u32, h: u32, px: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
        if p[3] == 0 {
            continue;
        }
        let (x, y) = ((i % w.max(1) as usize) as u32, (i / w.max(1) as usize) as u32);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + 1);
        y1 = y1.max(y + 1);
    }
    (x1 > x0 && y1 > y0 && y1 <= h).then_some((x0, y0, x1, y1))
}

/// Page `index` as a single-layer RGB 8-bit document named `name`: the page on a transparent
/// "Layer 1", `opts.ppi` pixels per inch, cropped to `opts.crop`.
pub fn render_page(bytes: &[u8], index: usize, opts: &PdfRaster, name: &str) -> Result<ImportResult, IoError> {
    let ppi = f64::from(opts.ppi);
    if !ppi.is_finite() || !(1.0..=30_000.0).contains(&ppi) {
        return Err(IoError::Unsupported(format!("resolution {} ppi is out of range (1–30000)", opts.ppi)));
    }
    let pdf = load(bytes)?;
    let p = page(&pdf, index)?;
    let scale = ppi / 72.0;
    let (w, h, px) = render(p, scale, TRANSPARENT)?;
    let (x0, y0, x1, y1) = match opts.crop {
        PdfCrop::BoundingBox => painted_bounds(w, h, &px).unwrap_or((0, 0, w, h)),
        crop => crop_pixels(p, crop, scale, (w, h)),
    };
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
    for y in y0..y1 {
        let row = (y as usize * w as usize + x0 as usize) * 4;
        out.extend_from_slice(px.get(row..row + cw as usize * 4).ok_or_else(|| IoError::Unsupported("rendered page is shorter than its size".into()))?);
    }
    let img = Image::from_raw(cw, ch, ChannelLayout::Rgba, photocraft_codecs::SampleType::U8, out)?;
    let mut r = crate::flat::image_to_document(name, &img)?;
    r.document.resolution_dpi = opts.ppi;
    r.document.icc_profile = Some(std::sync::Arc::new(photocraft_cms::Builtin::Srgb.profile().to_bytes().to_vec()));
    if let Some(l) = r.document.layers.first_mut() {
        l.name = "Layer 1".into();
    }
    let pages = pdf.pages().len();
    if pages > 1 {
        r.warnings.push(format!("page {} of {pages} opened", index + 1));
    }
    Ok(r)
}

/// Page `index` on white, scaled to fit `max_side` px (Import PDF's page previews).
pub fn thumbnail(bytes: &[u8], index: usize, max_side: u32) -> Result<Rgba8Image, IoError> {
    let pdf = load(bytes)?;
    let p = page(&pdf, index)?;
    let (w, h) = p.render_dimensions();
    let long = f64::from(w.max(h)).max(1.0);
    let (width, height, pixels) = render(p, f64::from(max_side.max(1)) / long, WHITE)?;
    Ok(Rgba8Image { width, height, pixels })
}

/// Opens a PDF without asking: PhotoCraft's own editing data when it is there, else the first
/// page at 300 ppi (the CLI, scripts and agents; the app asks with Import PDF first).
pub fn import(name: &str, bytes: &[u8]) -> Result<ImportResult, IoError> {
    if let Some(doc) = editable_document(bytes) {
        let mut document = doc?;
        document.name = name.to_string();
        return Ok(ImportResult { document, warnings: Vec::new(), source_read_only: false, preview_only: false });
    }
    let mut r = render_page(bytes, 0, &PdfRaster::default(), name)?;
    r.warnings.push("PDF rasterised at 300 ppi (vector content and text are now pixels)".into());
    Ok(r)
}

#[cfg(test)]
#[path = "pdf_import_tests.rs"]
mod tests;
