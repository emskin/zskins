use std::sync::Arc;

use gpui::{AnyElement, Image};

pub enum Preview {
    Text(String),
    Image(Arc<Image>),
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Layout {
    /// Single panel: list only.
    List,
    /// Wider panel split into a list on the left and a preview pane on the right.
    ListAndPreview,
}

/// A data source the launcher lists, filters, renders, and activates.
pub trait Source: 'static {
    fn name(&self) -> &'static str;
    /// Single Unicode glyph used in the source switcher bar.
    fn icon(&self) -> &'static str;
    fn placeholder(&self) -> &'static str;
    fn empty_text(&self) -> &'static str;

    fn filter(&self, query: &str) -> Vec<usize>;

    /// `selected` only drives content highlight; the row container, hover, and
    /// click handling live in the launcher.
    fn render_item(&self, ix: usize, selected: bool) -> AnyElement;

    fn activate(&self, ix: usize);

    /// Preview content for the given index. Only consulted when
    /// `layout()` is `ListAndPreview`.
    fn preview(&self, _ix: usize) -> Option<Preview> {
        None
    }

    /// Mime variants this entry was captured with. Returning ≥2 enables the
    /// secondary mime-list pane (Tab swaps the left column to it).
    fn mimes(&self, _ix: usize) -> Vec<String> {
        Vec::new()
    }

    /// Index into `mimes(ix)` of the variant `activate(ix)` defaults to. The
    /// launcher uses this to mark which row in the mime list is the implicit
    /// choice. Default: search by `primary_mime` string (override for O(1)).
    fn primary_mime_index(&self, ix: usize) -> Option<usize> {
        let primary = self.primary_mime(ix)?;
        self.mimes(ix).iter().position(|m| m == &primary)
    }

    /// The mime `activate(ix)` defaults to. Override at least one of this or
    /// `primary_mime_index`.
    fn primary_mime(&self, _ix: usize) -> Option<String> {
        None
    }

    /// Preview the entry under the chosen mime. Default delegates to `preview`.
    fn preview_for_mime(&self, ix: usize, _mime: &str) -> Option<Preview> {
        self.preview(ix)
    }

    /// Activate using a specific mime. Default delegates to `activate`.
    fn activate_with_mime(&self, ix: usize, _mime: &str) {
        self.activate(ix)
    }

    fn layout(&self) -> Layout {
        Layout::List
    }

    /// Optional notification rendered above the list (e.g. daemon-not-running
    /// warnings). The launcher reserves space for it; sources that don't need
    /// one return None.
    fn banner(&self) -> Option<AnyElement> {
        None
    }
}
