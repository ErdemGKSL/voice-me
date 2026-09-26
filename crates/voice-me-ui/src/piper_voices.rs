//! Settings → Piper voices (Story 3.15): browse the three online catalogs,
//! and download, delete or use a Piper voice.
//!
//! The view owns no network, disk or settings access. Every request goes to
//! the composition root as a [`PiperVoicesAction`] — fetching the catalogs,
//! downloading, deleting, using — and the root pushes the outcome back as a
//! [`PiperVoicesPanel`]. The catalogs are fetched only when the tab is
//! opened (the first time, [`PiperVoicesView::opened`]) or refreshed.
//!
//! One row per voice: every installed voice, then every catalog voice not
//! installed, filtered by language and by a search over the name, key and
//! language. The list is virtualized — the three catalogs together hold a
//! few hundred voices — with each row identified by its voice key.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    progress::Progress,
    scroll::Scrollbar,
    select::{Select, SelectEvent, SelectState},
    tag::Tag,
    v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, Size, Styled as _, Subscription,
    TestSupportExt as _, Window, div, px, size,
};
use voice_me_core::{CatalogResult, PiperCatalogEntry, assets::InstalledPiperVoice, format_bytes};

use crate::backend::error_line;

/// Something the user asked of the Piper voices. The root does it and
/// pushes the result back as a [`PiperVoicesPanel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiperVoicesAction {
    /// Fetch the three catalogs (again).
    Refresh,
    /// Download this voice.
    Download(PiperCatalogEntry),
    /// Delete the installed voice with this key.
    Delete(String),
    /// Make this voice Piper's language and voice.
    Use { key: String, locale: String },
}

/// Where the root sends each [`PiperVoicesAction`].
pub type PiperVoicesActions = Rc<dyn Fn(PiperVoicesAction, &mut App)>;

/// The catalogs, as far as the root has fetched them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PiperCatalogState {
    /// Not asked for yet this session.
    #[default]
    NotLoaded,
    /// Being fetched.
    Loading,
    /// Fetched: one result per source, a failed source as its own `Err`.
    Loaded(Vec<CatalogResult>),
}

/// Where one voice's download stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceDownload {
    /// Running; `total == 0` until the first figure arrives.
    Downloading { done: u64, total: u64 },
    /// The last download failed, and why.
    Failed(String),
}

/// Everything the tab shows, as the root last computed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PiperVoicesPanel {
    /// The voices in the cache, read from disk.
    pub installed: Vec<InstalledPiperVoice>,
    /// The voice Piper speaks in now, if one can be named.
    pub in_use: Option<String>,
    pub catalog: PiperCatalogState,
    pub downloads: HashMap<String, VoiceDownload>,
    /// A failed delete or "Use", shown above the list.
    pub error: Option<String>,
}

/// One row: a voice, installed or offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceRow {
    pub key: String,
    pub name: String,
    pub locale: String,
    pub language_label: String,
    pub quality: String,
    pub size_bytes: Option<u64>,
    pub source: String,
    pub licence: Option<String>,
    pub installed: bool,
    /// The catalog entry to download from, when a catalog lists it.
    pub entry: Option<PiperCatalogEntry>,
}

impl VoiceRow {
    /// "Turkish (Turkey) · medium · 63 MB · voice-me · CC0-1.0".
    fn facts(&self) -> String {
        let size = self
            .size_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "size unknown".to_string());
        let licence = self
            .licence
            .clone()
            .unwrap_or_else(|| "see model card".to_string());
        format!(
            "{} · {} · {size} · {} · {licence}",
            self.language_label, self.quality, self.source
        )
    }
}

/// Every row the panel implies: the installed voices, then each catalog
/// voice not installed, in catalog order.
pub fn voice_rows(panel: &PiperVoicesPanel) -> Vec<VoiceRow> {
    let catalog: Vec<&PiperCatalogEntry> = match &panel.catalog {
        PiperCatalogState::Loaded(results) => results
            .iter()
            .filter_map(|result| result.result.as_ref().ok())
            .flatten()
            .collect(),
        _ => Vec::new(),
    };
    let mut rows: Vec<VoiceRow> = panel
        .installed
        .iter()
        .map(|voice| VoiceRow {
            key: voice.key.clone(),
            name: voice.manifest.name.clone(),
            locale: voice.manifest.locale.clone(),
            language_label: voice.manifest.label.clone(),
            quality: voice.manifest.quality.clone(),
            size_bytes: Some(voice.size_bytes),
            source: voice.manifest.source.clone(),
            licence: voice.manifest.licence.clone(),
            installed: true,
            entry: catalog
                .iter()
                .find(|entry| entry.key == voice.key)
                .map(|entry| (*entry).clone()),
        })
        .collect();
    for entry in catalog {
        if rows.iter().any(|row| row.key == entry.key) {
            continue;
        }
        rows.push(VoiceRow {
            key: entry.key.clone(),
            name: entry.name.clone(),
            locale: entry.locale.clone(),
            language_label: entry.language_label.clone(),
            quality: entry.quality.clone(),
            size_bytes: entry.size_bytes,
            source: entry.source.label().to_string(),
            licence: entry.licence.clone(),
            installed: false,
            entry: Some(entry.clone()),
        });
    }
    rows
}

/// The rows the filter shows: `language` (a locale; `None` for all) and a
/// case-insensitive `query` over the name, key and language.
pub fn filter_rows(rows: &[VoiceRow], language: Option<&str>, query: &str) -> Vec<VoiceRow> {
    let query = query.trim().to_lowercase();
    rows.iter()
        .filter(|row| language.is_none_or(|locale| row.locale.eq_ignore_ascii_case(locale)))
        .filter(|row| {
            query.is_empty()
                || [&row.name, &row.key, &row.language_label, &row.locale]
                    .iter()
                    .any(|field| field.to_lowercase().contains(&query))
        })
        .cloned()
        .collect()
}

/// One entry of the language filter; the empty code is "All languages".
#[derive(Clone)]
struct LanguageFilter {
    code: String,
    label: SharedString,
}

impl gpui_kit::component::searchable_list::SearchableListItem for LanguageFilter {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.code
    }
}

/// "All languages", then every locale the rows have, by label.
fn language_filters(rows: &[VoiceRow]) -> Vec<LanguageFilter> {
    let mut languages: Vec<(String, String)> = Vec::new();
    for row in rows {
        if !languages.iter().any(|(code, _)| code == &row.locale) {
            languages.push((row.locale.clone(), row.language_label.clone()));
        }
    }
    languages.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    std::iter::once(LanguageFilter {
        code: String::new(),
        label: "All languages".into(),
    })
    .chain(languages.into_iter().map(|(code, label)| LanguageFilter {
        label: format!("{label} — {code}").into(),
        code,
    }))
    .collect()
}

/// A row's height in the virtualized list.
const ROW_HEIGHT: f32 = 64.;

/// The Piper voices tab.
pub struct PiperVoicesView {
    panel: PiperVoicesPanel,
    actions: PiperVoicesActions,
    search: Entity<InputState>,
    language_select: Entity<SelectState<Vec<LanguageFilter>>>,
    /// The locale filtered to, or `None` for all.
    language: Option<String>,
    /// The rows after filtering, rebuilt when the panel or filter changes.
    visible: Rc<Vec<VoiceRow>>,
    /// The language filter's items need rebuilding at the next render.
    filters_stale: bool,
    /// The list's scroll position. It has to outlive a render: without it
    /// `v_virtual_list` makes a new handle each frame and the list snaps
    /// back to the top, so it cannot be scrolled at all.
    scroll: VirtualListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl PiperVoicesView {
    pub fn new(
        panel: PiperVoicesPanel,
        actions: PiperVoicesActions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search voices"));
        let filters = language_filters(&voice_rows(&panel));
        let language_select =
            cx.new(|cx| SelectState::new(filters, Some(IndexPath::new(0)), window, cx));
        let subscriptions = vec![
            cx.subscribe(&search, |this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refilter(cx);
                }
            }),
            cx.subscribe(&language_select, |this, _select, event, cx| {
                let SelectEvent::Confirm(code) = event;
                this.language = code.clone().filter(|code| !code.is_empty());
                this.refilter(cx);
            }),
        ];
        let mut view = Self {
            panel,
            actions,
            search,
            language_select,
            language: None,
            visible: Rc::default(),
            filters_stale: false,
            scroll: VirtualListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        view.visible = Rc::new(view.filtered(cx));
        view
    }

    /// The tab was shown: the catalogs are fetched the first time.
    pub fn opened(&mut self, cx: &mut Context<Self>) {
        if self.panel.catalog == PiperCatalogState::NotLoaded {
            self.act(PiperVoicesAction::Refresh, cx);
        }
    }

    /// Replace what the tab shows.
    pub fn set_panel(&mut self, panel: PiperVoicesPanel, cx: &mut Context<Self>) {
        let codes = |panel: &PiperVoicesPanel| -> Vec<String> {
            language_filters(&voice_rows(panel))
                .into_iter()
                .map(|filter| filter.code)
                .collect()
        };
        let languages_changed = codes(&panel) != codes(&self.panel);
        self.filters_stale |= languages_changed;
        self.panel = panel;
        self.refilter(cx);
    }

    fn filtered(&self, cx: &App) -> Vec<VoiceRow> {
        let query = self.search.read(cx).value().to_string();
        filter_rows(&voice_rows(&self.panel), self.language.as_deref(), &query)
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.visible = Rc::new(self.filtered(cx));
        cx.notify();
    }

    fn act(&mut self, action: PiperVoicesAction, cx: &mut Context<Self>) {
        (self.actions.clone())(action, cx);
    }

    fn sync_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.filters_stale) {
            return;
        }
        let filters = language_filters(&voice_rows(&self.panel));
        let language = self.language.clone().unwrap_or_default();
        self.language_select.update(cx, |select, cx| {
            select.set_items(filters, window, cx);
            select.set_selected_value(&language, window, cx);
        });
    }

    fn row(&self, row: &VoiceRow, cx: &mut Context<Self>) -> AnyElement {
        let key = row.key.clone();
        let download = self.panel.downloads.get(&key).cloned();
        let downloading = match download {
            Some(VoiceDownload::Downloading { done, total }) => Some((done, total)),
            _ => None,
        };
        let failure = match download {
            Some(VoiceDownload::Failed(reason)) => Some(reason),
            _ => None,
        };
        let in_use = row.installed && self.panel.in_use.as_deref() == Some(key.as_str());
        let status = match (row.installed, in_use, downloading.is_some()) {
            (_, _, true) => "downloading",
            (true, true, _) => "in use",
            (true, false, _) => "installed",
            (false, _, _) => "not installed",
        };

        let id = |prefix: &str| SharedString::from(format!("{prefix}-{key}"));
        h_flex()
            .id(id("piper-voice-row"))
            .test_support()
            .h(px(ROW_HEIGHT))
            .items_center()
            .justify_between()
            .gap_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(row.name.clone()),
                            )
                            .child(
                                div()
                                    .id(id("piper-voice-status"))
                                    .test_support()
                                    .child(Tag::secondary().small().child(status)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(row.facts()),
                    )
                    .when_some(downloading, |el, (done, total)| {
                        let figure = if total > 0 {
                            format!("{} of {}", format_bytes(done), format_bytes(total))
                        } else {
                            "Starting…".to_string()
                        };
                        el.child(
                            h_flex()
                                .id(id("piper-voice-progress"))
                                .test_support()
                                .gap_2()
                                .items_center()
                                .child(
                                    div().w(px(200.)).child(
                                        Progress::new(id("piper-voice-progress-bar"))
                                            .loading(total == 0)
                                            .value(if total > 0 {
                                                (done as f64 / total as f64 * 100.) as f32
                                            } else {
                                                0.
                                            })
                                            .accessibility_label(format!(
                                                "Downloading {}: {figure}",
                                                row.name
                                            )),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(figure),
                                ),
                        )
                    })
                    .when_some(failure, |el, reason| {
                        el.child(error_line(id("piper-voice-error"), reason, cx))
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .when(row.installed, |el| {
                        let use_key = key.clone();
                        let locale = row.locale.clone();
                        let delete_key = key.clone();
                        el.child(
                            Button::new(id("piper-voice-use"))
                                .small()
                                .label("Use")
                                .disabled(in_use)
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.act(
                                        PiperVoicesAction::Use {
                                            key: use_key.clone(),
                                            locale: locale.clone(),
                                        },
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new(id("piper-voice-delete"))
                                .small()
                                .ghost()
                                .label("Delete")
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.act(PiperVoicesAction::Delete(delete_key.clone()), cx)
                                })),
                        )
                    })
                    .when_some(row.entry.clone().filter(|_| !row.installed), |el, entry| {
                        el.child(
                            Button::new(id("piper-voice-download"))
                                .small()
                                .primary()
                                .label("Download")
                                .disabled(downloading.is_some())
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.act(PiperVoicesAction::Download(entry.clone()), cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn rows_in(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let visible = self.visible.clone();
        range
            .filter_map(|index| visible.get(index))
            .map(|row| self.row(row, cx))
            .collect()
    }

    /// The catalog's state above the list: loading, and one line per
    /// source that failed.
    fn catalog_status(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match &self.panel.catalog {
            PiperCatalogState::NotLoaded => Vec::new(),
            PiperCatalogState::Loading => vec![
                div()
                    .id("piper-voices-loading")
                    .test_support()
                    .text_color(cx.theme().muted_foreground)
                    .child("Loading the voice catalogs…")
                    .into_any_element(),
            ],
            PiperCatalogState::Loaded(results) => results
                .iter()
                .filter_map(|result| {
                    let reason = result.result.as_ref().err()?;
                    Some(error_line(
                        format!("piper-voices-catalog-error-{}", result.source.label()),
                        format!(
                            "Couldn't list voices from {}: {reason}",
                            result.source.label()
                        ),
                        cx,
                    ))
                })
                .collect(),
        }
    }
}

impl Render for PiperVoicesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_filters(window, cx);
        let loading = self.panel.catalog == PiperCatalogState::Loading;
        let status = self.catalog_status(cx);
        let count = self.visible.len();
        let sizes: Rc<Vec<Size<Pixels>>> = Rc::new(vec![size(px(0.), px(ROW_HEIGHT)); count]);

        v_flex()
            .id("piper-voices-surface")
            .test_support()
            .size_full()
            .p_6()
            .gap_4()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().text_lg().child("Piper voices"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(240.)).child(Input::new(&self.search)))
                    .child(
                        Select::new(&self.language_select)
                            .id("piper-voices-language")
                            .accessibility_label("Language")
                            .menu_width(px(280.))
                            .w(px(240.)),
                    )
                    .child(
                        Button::new("piper-voices-refresh")
                            .ghost()
                            .label(if loading { "Loading…" } else { "Refresh" })
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.act(PiperVoicesAction::Refresh, cx)
                            })),
                    ),
            )
            .children(status)
            .when_some(self.panel.error.clone(), |el, error| {
                el.child(error_line("piper-voices-error", error, cx))
            })
            .map(|el| {
                if count == 0 {
                    el.child(
                        div()
                            .id("piper-voices-empty")
                            .test_support()
                            .text_color(cx.theme().muted_foreground)
                            .child(if loading {
                                ""
                            } else {
                                "No voices match. Clear the search, or pick all languages."
                            }),
                    )
                } else {
                    el.child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(
                                v_virtual_list(
                                    cx.entity(),
                                    "piper-voices-list",
                                    sizes,
                                    |this, range, _window, cx| this.rows_in(range, cx),
                                )
                                .track_scroll(&self.scroll)
                                .size_full(),
                            )
                            .child(Scrollbar::vertical(&self.scroll)),
                    )
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, component::Root};
    use voice_me_core::{PiperSource, assets::PiperVoiceManifest};

    use super::*;

    fn entry(key: &str, locale: &str, label: &str, source: PiperSource) -> PiperCatalogEntry {
        PiperCatalogEntry {
            key: key.to_string(),
            name: key.split('-').nth(1).unwrap_or(key).to_string(),
            locale: locale.to_string(),
            language_label: label.to_string(),
            quality: "medium".to_string(),
            licence: None,
            source,
            size_bytes: Some(63_000_000),
        }
    }

    fn installed(key: &str) -> InstalledPiperVoice {
        InstalledPiperVoice {
            key: key.to_string(),
            manifest: PiperVoiceManifest {
                name: "fahrettin".to_string(),
                locale: "tr_TR".to_string(),
                label: "Turkish (Turkey)".to_string(),
                quality: "medium".to_string(),
                source: "voice-me".to_string(),
                licence: Some("CC0-1.0".to_string()),
                model_sha256: None,
                config_sha256: None,
            },
            size_bytes: 63_206_316,
        }
    }

    fn panel() -> PiperVoicesPanel {
        PiperVoicesPanel {
            installed: vec![installed("tr_TR-fahrettin-medium")],
            in_use: Some("tr_TR-fahrettin-medium".to_string()),
            catalog: PiperCatalogState::Loaded(vec![
                CatalogResult {
                    source: PiperSource::VoiceMe,
                    result: Err("could not reach github".to_string()),
                },
                CatalogResult {
                    source: PiperSource::Official,
                    result: Ok(vec![
                        entry(
                            "tr_TR-dfki-medium",
                            "tr_TR",
                            "Turkish (Turkey)",
                            PiperSource::Official,
                        ),
                        entry(
                            "en_US-lessac-medium",
                            "en_US",
                            "English (United States)",
                            PiperSource::Official,
                        ),
                    ]),
                },
                CatalogResult {
                    source: PiperSource::Speaches,
                    result: Ok(vec![entry(
                        "tr_TR-fahrettin-medium",
                        "tr_TR",
                        "Turkish (TR)",
                        PiperSource::Speaches,
                    )]),
                },
            ]),
            downloads: HashMap::new(),
            error: None,
        }
    }

    #[test]
    fn installed_voices_come_first_and_are_not_listed_twice() {
        let rows = voice_rows(&panel());
        let keys: Vec<_> = rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "tr_TR-fahrettin-medium",
                "tr_TR-dfki-medium",
                "en_US-lessac-medium"
            ]
        );
        assert!(rows[0].installed);
        assert!(rows[0].facts().contains("CC0-1.0"));
        assert!(
            rows[1].facts().contains("see model card"),
            "{}",
            rows[1].facts()
        );
    }

    /// The Browse row: filter by language, and search.
    #[test]
    fn rows_filter_by_language_and_by_search() {
        let rows = voice_rows(&panel());
        let turkish = filter_rows(&rows, Some("tr_TR"), "");
        assert_eq!(turkish.len(), 2);
        let dfki = filter_rows(&rows, None, "DFKI");
        assert_eq!(dfki.len(), 1);
        assert_eq!(dfki[0].key, "tr_TR-dfki-medium");
        assert!(filter_rows(&rows, Some("en_US"), "dfki").is_empty());
        assert_eq!(filter_rows(&rows, None, "english").len(), 1);
    }

    fn open(
        cx: &mut TestAppContext,
        panel: PiperVoicesPanel,
    ) -> (
        gpui_kit::WindowHandle<Root>,
        Entity<PiperVoicesView>,
        Rc<RefCell<Vec<PiperVoicesAction>>>,
    ) {
        cx.update(gpui_kit::init);
        let sent: Rc<RefCell<Vec<PiperVoicesAction>>> = Rc::default();
        let actions: PiperVoicesActions = {
            let sent = sent.clone();
            Rc::new(move |action, _cx| sent.borrow_mut().push(action))
        };
        let mut slot = None;
        let handle = cx.open_window(size(px(900.), px(900.)), |window, cx| {
            let view = cx.new(|cx| PiperVoicesView::new(panel, actions, window, cx));
            slot = Some(view.clone());
            Root::new(view, window, cx)
        });
        (handle, slot.unwrap(), sent)
    }

    /// A long catalog scrolls, and stays scrolled on the next frame: the
    /// list's scroll handle lives in the view, not in one render.
    #[gpui_kit::test]
    fn a_long_voice_list_scrolls_and_stays_scrolled(cx: &mut TestAppContext) {
        let keys: Vec<String> = (0..60)
            .map(|n| format!("en_US-voice{n:02}-medium"))
            .collect();
        let panel = PiperVoicesPanel {
            installed: Vec::new(),
            in_use: None,
            catalog: PiperCatalogState::Loaded(vec![CatalogResult {
                source: PiperSource::Official,
                result: Ok(keys
                    .iter()
                    .map(|key| entry(key, "en_US", "English (US)", PiperSource::Official))
                    .collect()),
            }]),
            downloads: HashMap::new(),
            error: None,
        };
        let first = format!("piper-voice-row-{}", keys[0]);
        let (handle, _view, _sent) = open(cx, panel);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(first.clone()).is_some());
            window.scroll(
                first.clone(),
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-2000.))),
                cx,
            );
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(
                window.try_find(first.clone()).is_none(),
                "the first row scrolled out of view and stayed out"
            );
            assert!(
                window
                    .try_find(format!("piper-voice-row-{}", keys[40]))
                    .is_some(),
                "a row far down the list is shown"
            );
        })
        .unwrap();
    }

    /// An installed row shows Delete and Use, a catalog row shows Download,
    /// and a failed catalog is one inline line with the others still shown.
    #[gpui_kit::test]
    fn installed_rows_offer_delete_and_use_and_others_download(cx: &mut TestAppContext) {
        let (handle, _view, _sent) = open(cx, panel());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find("piper-voice-delete-tr_TR-fahrettin-medium")
                    .is_some()
            );
            assert!(
                window
                    .try_find("piper-voice-use-tr_TR-fahrettin-medium")
                    .is_some()
            );
            assert!(
                window
                    .try_find("piper-voice-download-tr_TR-fahrettin-medium")
                    .is_none()
            );
            assert!(
                window
                    .try_find("piper-voice-download-tr_TR-dfki-medium")
                    .is_some()
            );
            assert!(
                window
                    .try_find("piper-voice-delete-tr_TR-dfki-medium")
                    .is_none()
            );
            assert!(
                window
                    .try_find("piper-voices-catalog-error-voice-me")
                    .is_some()
            );
            assert!(
                window
                    .try_find("piper-voice-row-en_US-lessac-medium")
                    .is_some()
            );
        })
        .unwrap();
    }

    /// "Use" sends exactly one action, with the voice and its locale; and
    /// Download sends the catalog entry.
    #[gpui_kit::test]
    fn use_and_download_each_send_one_action(cx: &mut TestAppContext) {
        let mut panel = panel();
        panel.in_use = None;
        let (handle, _view, sent) = open(cx, panel);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("piper-voice-use-tr_TR-fahrettin-medium", cx);
        })
        .unwrap();
        assert_eq!(
            *sent.borrow(),
            vec![PiperVoicesAction::Use {
                key: "tr_TR-fahrettin-medium".to_string(),
                locale: "tr_TR".to_string()
            }]
        );
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("piper-voice-download-tr_TR-dfki-medium", cx);
        })
        .unwrap();
        assert!(matches!(
            sent.borrow().last(),
            Some(PiperVoicesAction::Download(entry)) if entry.key == "tr_TR-dfki-medium"
        ));
        assert_eq!(sent.borrow().len(), 2);
    }

    /// A download shows its progress, and its Download is disabled.
    #[gpui_kit::test]
    fn a_download_shows_progress(cx: &mut TestAppContext) {
        let (handle, view, sent) = open(cx, panel());
        view.update(cx, |view, cx| {
            let mut panel = panel();
            panel.downloads.insert(
                "tr_TR-dfki-medium".to_string(),
                VoiceDownload::Downloading {
                    done: 10_000_000,
                    total: 63_000_000,
                },
            );
            view.set_panel(panel, cx);
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find("piper-voice-progress-tr_TR-dfki-medium")
                    .is_some()
            );
            window.click("piper-voice-download-tr_TR-dfki-medium", cx);
        })
        .unwrap();
        assert!(
            sent.borrow().is_empty(),
            "a running download is not started twice"
        );
    }

    /// The catalogs are fetched when the tab is first opened, and only then.
    #[gpui_kit::test]
    fn opening_the_tab_fetches_the_catalogs_once(cx: &mut TestAppContext) {
        let (_handle, view, sent) = open(cx, PiperVoicesPanel::default());
        view.update(cx, |view, cx| view.opened(cx));
        assert_eq!(*sent.borrow(), vec![PiperVoicesAction::Refresh]);
        view.update(cx, |view, cx| {
            view.set_panel(
                PiperVoicesPanel {
                    catalog: PiperCatalogState::Loading,
                    ..PiperVoicesPanel::default()
                },
                cx,
            );
            view.opened(cx);
        });
        assert_eq!(sent.borrow().len(), 1);
    }
}
