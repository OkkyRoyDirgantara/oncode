//! Image files in a file tab: the picture instead of the text editor, with zoom. An SVG file can switch
//! between its picture and its source; the `view` tab parameter says which one a tab shows (see [`file_mode`]).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{v_flex, ActiveTheme as _};
use gpui_kit::{
    canvas, div, fill, img, point, px, size, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Hsla, Image, ImageFormat, ImageSource,
    InteractiveElement as _, IntoElement, MouseButton, ObjectFit, ParentElement as _, PinchEvent, Pixels, Point, Render, RenderImage, ScrollEvent, ScrollHandle, SharedString,
    Size, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, SvgRenderer, Task, Window, SMOOTH_SVG_SCALE_FACTOR,
};
use oncode_core::api::fs;
use serde_json::Value;

use super::active_image;
use super::buffer::{is_untitled, Buffer, BufferEvent, BufferStatus};
use crate::commands::Command;
use crate::model::TabData;
use crate::spawn_core;
use crate::workbench::{EditorAction, Env, Workbench};
use crate::Lucide;

/// The tab parameter that overrides how a file tab shows its file: `"text"` or `"image"`.
pub const VIEW_PARAM: &str = "view";

/// Extensions of the files shown as a picture.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "jpe", "jfif", "gif", "webp", "bmp", "ico", "tif", "tiff", "pbm", "pgm", "ppm", "pnm", "svg"];

/// The longest side, in pixels, of a picture handed to the GPU (GPUI rasterizes SVG up to this size too).
/// A larger picture is scaled down for display; its real size is still reported.
const MAX_TEXTURE_SIDE: u32 = 8192;

/// Space around the picture.
const PADDING: f32 = 16.;
/// The side of one checkerboard square behind transparent pictures.
const CHECKER_CELL: f32 = 8.;
const MIN_SCALE: f32 = 0.01;
const MAX_SCALE: f32 = 20.;
/// The steps of "Zoom In" and "Zoom Out".
const ZOOM_LEVELS: [f32; 18] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1., 1.5, 2., 3., 5., 7., 10., 15., 20.];
/// Delay after the last edit of an SVG source before its picture is redrawn.
const REFRESH_DELAY: Duration = Duration::from_millis(150);
/// Delay after the last zoom change before an SVG is rasterized again at the new scale.
const RERASTER_DELAY: Duration = Duration::from_millis(120);

/// How a file tab shows its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Text,
    Image,
}

fn extension(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?;
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty()).then(|| extension.to_ascii_lowercase())
}

/// The file is shown as a picture by default.
pub fn is_image(path: &str) -> bool {
    !is_untitled(path) && extension(path).is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension.as_str()))
}

/// An SVG file: a picture that can also be shown as text.
pub fn is_svg(path: &str) -> bool {
    !is_untitled(path) && extension(path).as_deref() == Some("svg")
}

/// How this file tab shows its file: as its `view` parameter says, otherwise as a picture if it is an image.
pub fn file_mode(tab: &TabData) -> FileMode {
    let image = tab.param_str("path").is_some_and(is_image);
    match tab.param_str(VIEW_PARAM) {
        Some("text") => FileMode::Text,
        _ if image => FileMode::Image,
        _ => FileMode::Text,
    }
}

/// Makes a file tab show its file this way.
pub fn set_file_mode(tab: &mut TabData, mode: FileMode) {
    let value = match mode {
        FileMode::Text => "text",
        FileMode::Image => "image",
    };
    tab.params[VIEW_PARAM] = Value::from(value);
}

/// A file tab of an SVG file, which can show either its picture or its text.
fn is_svg_tab(tab: &TabData) -> bool {
    tab.kind == "file" && tab.param_str("path").is_some_and(is_svg)
}

/// Commands and tab bar buttons for pictures: switching an SVG between picture and text, and zooming.
pub(crate) fn register_commands(workbench: &mut Workbench, cx: &mut Context<Workbench>) {
    let weak = cx.weak_entity();
    let svg_shown_as = |mode: FileMode| {
        let weak = weak.clone();
        move |_: &Window, cx: &App| weak.upgrade().is_some_and(|w| w.read(cx).model().active().is_some_and(|(_, tab)| is_svg_tab(tab) && file_mode(tab) == mode))
    };
    let show_as = |mode: FileMode| {
        let weak = weak.clone();
        move |_: &Value, window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |workbench, cx| {
                let Some((group, id)) = workbench.model().active().filter(|(_, tab)| is_svg_tab(tab)).map(|(group, tab)| (group.id, tab.id.clone())) else { return };
                // Only this group's tab: the same file can show its picture in one group and its text in another.
                workbench.update_tab(group, &id, |tab| set_file_mode(tab, mode), window, cx);
            });
        }
    };
    workbench.register_command(Command::new("image.openAsText", "Open as Text", show_as(FileMode::Text)).category("Image").when(svg_shown_as(FileMode::Image)));
    workbench.register_command(Command::new("image.openAsImage", "Open as Image", show_as(FileMode::Image)).category("Image").when(svg_shown_as(FileMode::Text)));
    workbench.register_editor_action(EditorAction::new("image-open-as-text", Lucide::CodeXml, "Open as Text", "image.openAsText", |tab, _| {
        is_svg_tab(tab) && file_mode(tab) == FileMode::Image
    }));
    workbench.register_editor_action(EditorAction::new("image-open-as-image", Lucide::Image, "Open as Image", "image.openAsImage", |tab, _| {
        is_svg_tab(tab) && file_mode(tab) == FileMode::Text
    }));

    // Zooming the picture in the active tab; the keys only while the picture has focus.
    let has_image = {
        let weak = weak.clone();
        move |_: &Window, cx: &App| weak.upgrade().is_some_and(|w| active_image(w.read(cx), cx).is_some())
    };
    let image_focused = {
        let weak = weak.clone();
        move |window: &Window, cx: &App| weak.upgrade().and_then(|w| active_image(w.read(cx), cx)).is_some_and(|image| image.focus_handle(cx).contains_focused(window, cx))
    };
    let zoom = |f: fn(&mut ImageView, &mut Window, &mut Context<ImageView>)| {
        let weak = weak.clone();
        move |_: &Value, window: &mut Window, cx: &mut App| {
            let Some(image) = weak.upgrade().and_then(|w| active_image(w.read(cx), cx)) else { return };
            image.update(cx, |image, cx| f(image, window, cx));
        }
    };
    let commands = [
        ("image.zoomIn", "Zoom In", "Mod+=", zoom(ImageView::zoom_in)),
        ("image.zoomOut", "Zoom Out", "Mod+-", zoom(ImageView::zoom_out)),
        ("image.resetZoom", "Zoom to Fit", "Mod+0", zoom(ImageView::reset_zoom)),
    ];
    for (id, title, key, run) in commands {
        workbench.register_command(Command::new(id, title, run).category("Image").key(key).when(has_image.clone()).keys_when(image_focused.clone()));
    }
}

/// The format by the file's first bytes, otherwise by its extension.
fn format_of(path: &str, bytes: &[u8]) -> Option<ImageFormat> {
    if is_svg(path) {
        return Some(ImageFormat::Svg);
    }
    let sniffed = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        Some(ImageFormat::Ico)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else if bytes.len() >= 2 && bytes[0] == b'P' && (b'1'..=b'6').contains(&bytes[1]) {
        Some(ImageFormat::Pnm)
    } else {
        None
    };
    sniffed.or_else(|| match extension(path)?.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" | "jpe" | "jfif" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::Webp),
        "bmp" => Some(ImageFormat::Bmp),
        "ico" => Some(ImageFormat::Ico),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "pbm" | "pgm" | "ppm" | "pnm" => Some(ImageFormat::Pnm),
        _ => None,
    })
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Webp => "WebP",
        ImageFormat::Gif => "GIF",
        ImageFormat::Svg => "SVG",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Ico => "ICO",
        ImageFormat::Pnm => "PNM",
    }
}

/// A decoded picture.
struct Picture {
    image: Arc<RenderImage>,
    /// The size at 100 %, in pixels.
    width: f32,
    height: f32,
    /// The size of the file (for SVG: of its source as currently edited).
    bytes: usize,
    format: ImageFormat,
    /// SVG only: the source and the scale `image` was rasterized at, to rasterize it again when zooming needs it.
    svg: Option<(Arc<[u8]>, f32)>,
}

/// Information about the picture shown, for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub format: &'static str,
}

fn decode(path: &str, bytes: Vec<u8>, svg_scale: f32, renderer: &SvgRenderer) -> Result<Picture, String> {
    let len = bytes.len();
    let format = format_of(path, &bytes).ok_or_else(|| "this image format is not supported".to_string())?;
    if format == ImageFormat::Svg {
        let source: Arc<[u8]> = bytes.into();
        let (image, width, height, scale) = rasterize_svg(&source, svg_scale, renderer)?;
        return Ok(Picture { image, width, height, bytes: len, format, svg: Some((source, scale)) });
    }
    let image = Image::from_bytes(format, bytes).to_image_data(renderer.clone()).map_err(|error| format!("{error:#}"))?;
    let size = image.size(0);
    let (width, height) = (u32::from(size.width), u32::from(size.height));
    if width == 0 || height == 0 {
        return Err("the image is empty".to_string());
    }
    Ok(Picture { image: fit_texture(image), width: width as f32, height: height as f32, bytes: len, format, svg: None })
}

/// Rasterizes an SVG at `scale` (capped so the result fits in a texture); returns the image, the SVG's own size,
/// and the scale actually used.
fn rasterize_svg(source: &[u8], scale: f32, renderer: &SvgRenderer) -> Result<(Arc<RenderImage>, f32, f32, f32), String> {
    let tree = usvg::Tree::from_data(source, &usvg::Options::default()).map_err(|error| error.to_string())?;
    let (width, height) = (tree.size().width(), tree.size().height());
    let scale = clamp_svg_scale(scale, width, height);
    let image = renderer.render_single_frame(source, scale).map_err(|error| error.to_string())?;
    Ok((image, width, height, scale))
}

/// The SVG raster scale (GPUI renders at `scale` × [`SMOOTH_SVG_SCALE_FACTOR`]) for a picture shown at
/// `display_scale` on a screen with `window_scale`: the next power of two, so small zoom steps reuse the raster.
/// At least 1, the scale of the first load, so a picture shown at its size or smaller is rasterized once.
fn svg_scale_for(display_scale: f32, window_scale: f32) -> f32 {
    let needed = (display_scale * window_scale / SMOOTH_SVG_SCALE_FACTOR).max(1.);
    2f32.powf(needed.log2().ceil())
}

fn clamp_svg_scale(scale: f32, width: f32, height: f32) -> f32 {
    let largest = width.max(height).max(1.);
    scale.min(MAX_TEXTURE_SIDE as f32 / (SMOOTH_SVG_SCALE_FACTOR * largest))
}

/// Scales every frame down so the longest side is at most [`MAX_TEXTURE_SIDE`].
fn fit_texture(picture: Arc<RenderImage>) -> Arc<RenderImage> {
    let largest = |size: Size<gpui_kit::DevicePixels>| u32::from(size.width).max(u32::from(size.height));
    if (0..picture.frame_count()).all(|frame| largest(picture.size(frame)) <= MAX_TEXTURE_SIDE) {
        return picture;
    }
    let frames: Option<Vec<image::Frame>> = (0..picture.frame_count())
        .map(|frame| {
            let size = picture.size(frame);
            let (width, height) = (u32::from(size.width), u32::from(size.height));
            let buffer = image::RgbaImage::from_raw(width, height, picture.as_bytes(frame)?.to_vec())?;
            let ratio = MAX_TEXTURE_SIDE as f64 / width.max(height) as f64;
            let (width, height) = (((width as f64 * ratio).round() as u32).max(1), ((height as f64 * ratio).round() as u32).max(1));
            // The pixels are BGRA; resizing treats the four channels alike, so the order does not matter.
            let resized = image::imageops::resize(&buffer, width, height, image::imageops::FilterType::Triangle);
            Some(image::Frame::from_parts(resized, 0, 0, picture.delay(frame)))
        })
        .collect();
    match frames {
        Some(frames) => Arc::new(RenderImage::new(frames)),
        None => picture,
    }
}

/// How large the picture is shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    /// The whole picture fits in the view; a small picture is not enlarged.
    Fit,
    /// A fixed scale (1.0 = actual size).
    Scale(f32),
}

enum Content {
    Loading,
    Ready(Picture),
    Failed(SharedString),
}

/// The picture of an image file, kept up to date with the file (and, for SVG, with unsaved edits of its source).
pub struct ImageView {
    env: Env,
    path: String,
    buffer: Entity<Buffer>,
    focus_handle: FocusHandle,
    content: Content,
    zoom: Zoom,
    /// The size of the area the picture is shown in, as of the last frame.
    viewport: Rc<Cell<Option<Size<Pixels>>>>,
    scroll: ScrollHandle,
    /// Bumped by every load, so the result of an older, slower load is dropped.
    generation: u64,
    /// The SVG raster scale being prepared.
    pending_svg_scale: Option<f32>,
    _load: Task<()>,
    _reraster: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ImageView {
    pub fn new(path: &str, env: Env, buffer: Entity<Buffer>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.subscribe_in(&buffer, window, |this, _, event: &BufferEvent, window, cx| match event {
                BufferEvent::Loaded => this.load(window, cx),
                // An SVG edited as text, e.g. in another group.
                BufferEvent::Edited => this.schedule_load(window, cx),
                BufferEvent::Saved => {}
            }),
            // The picture lives in the GPU texture atlas until it is removed explicitly.
            cx.on_release(|this, cx| {
                if let Content::Ready(picture) = &this.content {
                    let image = picture.image.clone();
                    cx.defer(move |cx| cx.drop_image(image, None));
                }
            }),
        ];
        let mut this = Self {
            env,
            path: path.to_string(),
            buffer,
            focus_handle: cx.focus_handle(),
            content: Content::Loading,
            zoom: Zoom::Fit,
            viewport: Rc::default(),
            scroll: ScrollHandle::new(),
            generation: 0,
            pending_svg_scale: None,
            _load: Task::ready(()),
            _reraster: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.load(window, cx);
        this
    }

    /// The size, file size, and format of the picture, once it is shown.
    pub fn info(&self) -> Option<ImageInfo> {
        let Content::Ready(picture) = &self.content else { return None };
        Some(ImageInfo { width: picture.width.round() as u32, height: picture.height.round() as u32, bytes: picture.bytes, format: format_name(picture.format) })
    }

    /// Why the picture cannot be shown.
    pub fn error(&self) -> Option<&str> {
        match &self.content {
            Content::Failed(message) => Some(message),
            _ => None,
        }
    }

    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// The scale the picture is shown at (1.0 = actual size); `None` until it is loaded and laid out.
    pub fn scale(&self) -> Option<f32> {
        let Content::Ready(picture) = &self.content else { return None };
        match self.zoom {
            Zoom::Scale(scale) => Some(scale),
            Zoom::Fit => {
                let viewport = self.viewport.get()?;
                let room = |side: Pixels| (f32::from(side) - 2. * PADDING).max(1.);
                Some((room(viewport.width) / picture.width).min(room(viewport.height) / picture.height).clamp(MIN_SCALE, 1.))
            }
        }
    }

    /// The scale an SVG picture is currently rasterized at; `None` for other pictures.
    pub fn svg_raster_scale(&self) -> Option<f32> {
        match &self.content {
            Content::Ready(Picture { svg: Some((_, scale)), .. }) => Some(*scale),
            _ => None,
        }
    }

    pub fn zoom_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.scale().unwrap_or(1.);
        let next = ZOOM_LEVELS.iter().copied().find(|level| *level > current * 1.001).unwrap_or(MAX_SCALE);
        self.zoom_to(Zoom::Scale(next), None, window, cx);
    }

    pub fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.scale().unwrap_or(1.);
        let next = ZOOM_LEVELS.iter().rev().copied().find(|level| *level < current * 0.999).unwrap_or(current.min(ZOOM_LEVELS[0]));
        self.zoom_to(Zoom::Scale(next), None, window, cx);
    }

    /// Back to fitting the whole picture in the view.
    pub fn reset_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom_to(Zoom::Fit, None, window, cx);
    }

    /// Switches between fitting the view and the actual size (the zoom button in the status bar).
    pub fn toggle_actual_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let zoom = if self.zoom == Zoom::Fit { Zoom::Scale(1.) } else { Zoom::Fit };
        self.zoom_to(zoom, None, window, cx);
    }

    /// Changes the zoom, keeping the point at `anchor` (relative to the view; default: its center) in place.
    fn zoom_to(&mut self, zoom: Zoom, anchor: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.scale();
        self.zoom = match zoom {
            Zoom::Scale(scale) => Zoom::Scale(scale.clamp(MIN_SCALE, MAX_SCALE)),
            Zoom::Fit => Zoom::Fit,
        };
        if let (Some(before), Some(after), Some(viewport), Content::Ready(picture)) = (before, self.scale(), self.viewport.get(), &self.content) {
            let viewport = (f32::from(viewport.width), f32::from(viewport.height));
            let anchor = anchor.map_or((viewport.0 / 2., viewport.1 / 2.), |anchor| (f32::from(anchor.x), f32::from(anchor.y)));
            let offset = self.scroll.offset();
            let keep = |anchor: f32, offset: f32, viewport: f32, side: f32| {
                // The picture is centered in content at least as large as the view.
                let origin = |scale: f32| (content_side(viewport, side * scale) - side * scale) / 2.;
                let on_picture = (anchor - offset - origin(before)) / before;
                let max = content_side(viewport, side * after) - viewport;
                (anchor - origin(after) - on_picture * after).clamp(-max.max(0.), 0.)
            };
            let x = keep(anchor.0, f32::from(offset.x), viewport.0, picture.width);
            let y = keep(anchor.1, f32::from(offset.y), viewport.1, picture.height);
            self.scroll.set_offset(point(px(x), px(y)));
        }
        self.update_svg_scale(window, cx);
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Ctrl (Cmd on macOS) + wheel zooms; a plain wheel scrolls.
        if !event.modifiers.secondary(cx) {
            return;
        }
        cx.stop_propagation();
        let Some(scale) = self.scale() else { return };
        let delta = f32::from(event.delta.pixel_delta(px(20.)).y);
        let anchor = event.position - self.scroll.bounds().origin;
        self.zoom_to(Zoom::Scale(scale * (delta * 0.01).exp()), Some(anchor), window, cx);
    }

    fn on_pinch(&mut self, event: &PinchEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scale) = self.scale() else { return };
        let anchor = event.position - self.scroll.bounds().origin;
        self.zoom_to(Zoom::Scale(scale * (1. + event.delta)), Some(anchor), window, cx);
    }

    fn schedule_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._load = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(REFRESH_DELAY).await;
            let _ = this.update_in(cx, |this, window, cx| this.load(window, cx));
        });
    }

    /// Reads the picture again: an SVG from its buffer (with unsaved edits), any other image from disk.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let buffer = self.buffer.read(cx);
        let from_buffer = match buffer.status() {
            // The file view shows these states itself.
            BufferStatus::Loading | BufferStatus::TooLarge { .. } | BufferStatus::Error(_) => return,
            BufferStatus::Ready if is_svg(&self.path) => match buffer.text(cx) {
                Some(text) => Some(text.to_string().into_bytes()),
                None => return,
            },
            _ => None,
        };
        let bytes: Task<Result<Vec<u8>, String>> = match from_buffer {
            Some(bytes) => Task::ready(Ok(bytes)),
            None => {
                let (core, label, path) = (self.env.core.clone(), self.env.window.to_string(), self.path.clone());
                let read = spawn_core(cx, async move { fs::fs_read_bytes(&core, &label, path).await });
                cx.background_spawn(async move { read.await.map_err(|error| error.message) })
            }
        };
        self.generation += 1;
        let generation = self.generation;
        let svg_scale = match &self.content {
            Content::Ready(Picture { svg: Some((_, scale)), .. }) => *scale,
            _ => 1.,
        };
        let (path, renderer) = (self.path.clone(), cx.svg_renderer());
        self._load = cx.spawn_in(window, async move |this, cx| {
            let decoded = match bytes.await {
                Ok(bytes) => cx.background_spawn(async move { decode(&path, bytes, svg_scale, &renderer) }).await,
                Err(error) => Err(error),
            };
            let _ = this.update_in(cx, |this, window, cx| this.install(generation, decoded, window, cx));
        });
    }

    fn install(&mut self, generation: u64, decoded: Result<Picture, String>, window: &mut Window, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        let name = self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path);
        let content = match decoded {
            Ok(picture) => Content::Ready(picture),
            Err(error) => Content::Failed(format!("{name} cannot be shown as an image: {error}").into()),
        };
        if let Content::Ready(old) = std::mem::replace(&mut self.content, content) {
            cx.drop_image(old.image, Some(window));
        }
        self.pending_svg_scale = None;
        self.update_svg_scale(window, cx);
        cx.notify();
    }

    /// Rasterizes an SVG again (shortly) if it is shown at a scale its current raster does not suit.
    fn update_svg_scale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scale) = self.scale() else { return };
        let Content::Ready(Picture { svg: Some((source, current)), width, height, .. }) = &self.content else { return };
        let wanted = clamp_svg_scale(svg_scale_for(scale, window.scale_factor()), *width, *height);
        if (wanted - *current).abs() < 1e-4 {
            self.pending_svg_scale = None;
            self._reraster = Task::ready(());
            return;
        }
        if self.pending_svg_scale == Some(wanted) {
            return;
        }
        self.pending_svg_scale = Some(wanted);
        let (source, renderer) = (source.clone(), cx.svg_renderer());
        self._reraster = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(RERASTER_DELAY).await;
            let raster = {
                let source = source.clone();
                cx.background_spawn(async move { rasterize_svg(&source, wanted, &renderer) }).await
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending_svg_scale = None;
                let Ok((image, _, _, scale)) = raster else { return };
                let Content::Ready(picture) = &mut this.content else { return };
                // The source changed meanwhile: its own load brings a new raster.
                if !picture.svg.as_ref().is_some_and(|(current, _)| Arc::ptr_eq(current, &source)) {
                    return;
                }
                let old = std::mem::replace(&mut picture.image, image);
                picture.svg = Some((source, scale));
                cx.drop_image(old, Some(window));
                cx.notify();
            });
        });
    }

    fn render_picture(&self, picture: &Picture, scale: f32, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let viewport = self.viewport.get().unwrap_or_default();
        let (width, height) = (picture.width * scale, picture.height * scale);
        let (content_width, content_height) = (content_side(f32::from(viewport.width), width), content_side(f32::from(viewport.height), height));
        let theme = cx.theme();
        // JPEG has no transparency; anything else may.
        let checker = (picture.format != ImageFormat::Jpeg).then(|| checkerboard(theme.background, theme.foreground.opacity(0.08)));
        div()
            .id("image-scroll")
            .size_full()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .id("image-content")
                    .relative()
                    .flex_none()
                    .w(px(content_width))
                    .h(px(content_height))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .on_pinch(cx.listener(Self::on_pinch))
                    .child(
                        div()
                            .absolute()
                            .left(px((content_width - width) / 2.))
                            .top(px((content_height - height) / 2.))
                            .w(px(width))
                            .h(px(height))
                            .children(checker)
                            .child(
                                // The id keeps the frame state of an animated GIF; `Fill` because the box already has the picture's proportions.
                                img(ImageSource::Render(picture.image.clone())).id("image-picture").object_fit(ObjectFit::Fill).absolute().top_0().left_0().size_full(),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// The length of the scrollable content along one side: the picture plus padding, at least the view.
fn content_side(viewport: f32, picture: f32) -> f32 {
    (picture + 2. * PADDING).max(viewport)
}

/// A checkerboard filling its parent, so transparent parts of a picture are visible on any theme.
/// Only the squares inside the visible area are painted.
fn checkerboard(light: Hsla, dark: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.paint_quad(fill(bounds, light));
            let visible = bounds.intersect(&window.content_mask().bounds);
            if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
                return;
            }
            let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let cells = |from: Pixels, to: Pixels, start: f32| ((f32::from(from) - start) / CHECKER_CELL).floor() as i64..((f32::from(to) - start) / CHECKER_CELL).ceil() as i64;
            for row in cells(visible.top(), visible.bottom(), top) {
                for column in cells(visible.left(), visible.right(), left) {
                    if (row + column) % 2 == 0 {
                        continue;
                    }
                    let origin = point(px(left + column as f32 * CHECKER_CELL), px(top + row as f32 * CHECKER_CELL));
                    window.paint_quad(fill(Bounds::new(origin, size(px(CHECKER_CELL), px(CHECKER_CELL))).intersect(&bounds), dark));
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

impl Focusable for ImageView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // In "Fit", the scale follows the size of the view.
        self.update_svg_scale(window, cx);
        let viewport = self.viewport.clone();
        let view = cx.entity_id();
        let measure = canvas(
            move |bounds, _, cx| {
                if viewport.get() != Some(bounds.size) {
                    viewport.set(Some(bounds.size));
                    cx.notify(view);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let body = match &self.content {
            Content::Loading => v_flex().size_full().items_center().justify_center().child(Spinner::new()).into_any_element(),
            Content::Failed(message) => {
                v_flex().size_full().items_center().justify_center().px_6().text_color(cx.theme().muted_foreground).child(div().child(message.clone())).into_any_element()
            }
            Content::Ready(picture) => match self.scale() {
                Some(scale) => self.render_picture(picture, scale, cx),
                // Laid out for the first time: the size of the view is known after this frame.
                None => div().into_any_element(),
            },
        };
        div()
            .id("image-view")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.focus_handle.focus(window, cx)))
            .child(measure)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tab(path: &str, view: Option<&str>) -> TabData {
        let mut params = json!({ "path": path });
        if let Some(view) = view {
            params[VIEW_PARAM] = json!(view);
        }
        TabData::new("file", path, params)
    }

    #[test]
    fn image_files_are_recognized_by_extension() {
        for path in ["/p/a.png", "/p/B.JPG", "/p/c.jpeg", "/p/d.gif", "/p/e.webp", "/p/f.svg", "C:\\p\\g.ico", "/p/h.tiff", "/p/i.bmp"] {
            assert!(is_image(path), "{path}");
        }
        for path in ["/p/a.txt", "/p/png", "/p/.png", "/p.png/readme", "/p/a.svg.txt", "untitled:Untitled-1.png"] {
            assert!(!is_image(path), "{path}");
        }
        assert!(is_svg("/p/logo.SVG") && !is_svg("/p/logo.png"));
    }

    #[test]
    fn a_file_tab_shows_images_as_pictures_unless_asked_for_text() {
        assert_eq!(file_mode(&tab("/p/a.png", None)), FileMode::Image);
        assert_eq!(file_mode(&tab("/p/a.svg", None)), FileMode::Image);
        assert_eq!(file_mode(&tab("/p/a.svg", Some("text"))), FileMode::Text);
        assert_eq!(file_mode(&tab("/p/a.svg", Some("image"))), FileMode::Image);
        assert_eq!(file_mode(&tab("/p/a.rs", None)), FileMode::Text);
        assert_eq!(file_mode(&tab("/p/a.rs", Some("image"))), FileMode::Text, "only images can be pictures");

        let mut svg = tab("/p/a.svg", None);
        set_file_mode(&mut svg, FileMode::Text);
        assert_eq!(file_mode(&svg), FileMode::Text);
        set_file_mode(&mut svg, FileMode::Image);
        assert_eq!(file_mode(&svg), FileMode::Image);
    }

    #[test]
    fn the_format_comes_from_the_content_before_the_extension() {
        assert_eq!(format_of("/p/a.png", b"\x89PNG\r\n\x1a\n...."), Some(ImageFormat::Png));
        // A JPEG saved with a .png name still decodes.
        assert_eq!(format_of("/p/a.png", &[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageFormat::Jpeg));
        assert_eq!(format_of("/p/a.webp", b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(format_of("/p/a.gif", b"GIF89a"), Some(ImageFormat::Gif));
        assert_eq!(format_of("/p/a.svg", b"<svg/>"), Some(ImageFormat::Svg));
        assert_eq!(format_of("/p/a.tif", b"unknown"), Some(ImageFormat::Tiff), "falls back to the extension");
        assert_eq!(format_of("/p/a.dat", b"unknown"), None);
    }

    #[test]
    fn svg_raster_scale_steps_in_powers_of_two_and_fits_a_texture() {
        assert_eq!(svg_scale_for(1., 1.), 1., "GPUI already rasterizes at twice the size");
        assert_eq!(svg_scale_for(2., 1.), 1.);
        assert_eq!(svg_scale_for(3., 1.), 2.);
        assert_eq!(svg_scale_for(1., 2.), 1.);
        assert_eq!(svg_scale_for(10., 2.), 16.);
        assert_eq!(svg_scale_for(0.2, 1.), 1., "not below the first load's scale");
        assert_eq!(clamp_svg_scale(16., 1024., 512.), 4., "8192 px at most");
        assert_eq!(clamp_svg_scale(1., 24., 24.), 1.);
    }

    #[test]
    fn oversized_pictures_are_scaled_down_for_the_gpu() {
        let frame = image::Frame::new(image::RgbaImage::new(MAX_TEXTURE_SIDE * 2, 10));
        let picture = fit_texture(Arc::new(RenderImage::new(vec![frame])));
        let size = picture.size(0);
        assert_eq!((u32::from(size.width), u32::from(size.height)), (MAX_TEXTURE_SIDE, 5));

        let small = Arc::new(RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(64, 32))]));
        assert!(Arc::ptr_eq(&fit_texture(small.clone()), &small), "left as is");
    }

    #[test]
    fn content_keeps_padding_around_large_pictures_and_fills_the_view_for_small_ones() {
        assert_eq!(content_side(800., 100.), 800.);
        assert_eq!(content_side(800., 1000.), 1000. + 2. * PADDING);
    }
}
