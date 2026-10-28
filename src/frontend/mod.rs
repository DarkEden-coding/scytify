//! Event-driven native library browser and player. Backend state is authoritative.
mod artwork;
mod chrome;
mod devices;
mod executor;
mod icons;
mod layout;
mod library;
mod lyrics;
mod measure;
mod navigation;
mod paging;
mod pending_play;
mod persistence;
mod search;
mod shortcuts;
mod sidebar;
mod smooth_scroll;
mod splits;
mod styles;
pub mod theme;
mod tint;
mod track_filter;
mod view;
use crate::{app_icon, contracts::*, domain::*};
use iced::futures::SinkExt;
use iced::{Subscription, Task, Theme, window};
use navigation::{History, Route, is_search};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};
use theme::Palette;

type Element<'a> = iced::Element<'a, Message>;

/// Saved albums requested for the Home shelf; more than one row at any window width.
const SHELF_LIMIT: u16 = 20;

/// Widget ids targeted by focus and scroll operations.
const SEARCH_INPUT: &str = "search";
const MAIN_SCROLL: &str = "main";
/// How long a notice stays before it dismisses itself.
const NOTICE_DURATION: Duration = Duration::from_secs(6);
/// Lyrics redraw interval while playing. Each redraw rebuilds the whole window, and 30
/// per second keeps the syllable fill and line glide smooth at half the display rate.
const LYRICS_FRAME: Duration = Duration::from_millis(33);
/// How close the reported position must come to a released seek for the slider to follow it.
const SEEK_REACHED_MS: u64 = 1000;

#[derive(Clone, Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "The bounded UI stream avoids allocating each state update separately."
)]
enum Message {
    State(AppState),
    Backend(Event),
    Disconnected,
    Window(window::Id, window::Event),
    Opened(window::Id),
    Action(Action),
    SidebarFilter(String),
    /// Sidebar scroll offset, viewport height and whether the end is near.
    SidebarScroll(f32, f32, bool),
    SidebarMore,
    Search(String),
    SearchNow,
    SearchDebounced(u64),
    FocusSearch,
    /// Sidebar "Search": open the search page for the current text and focus the field.
    OpenSearch,
    Navigate(Route),
    RetryPage,
    Back,
    Forward,
    /// Main view scroll offset and viewport height.
    Scroll(f32, f32),
    MainResized(iced::Size),
    HoverRow(usize, bool),
    SelectRow(usize),
    TrackFilter(String),
    TrackSort(track_filter::Sort),
    TrackMore,
    RowMove(bool),
    PlaySelected,
    TrackMenu(Box<Track>),
    CloseTrackMenu,
    CopyLink(String),
    RetryShelves,
    TogglePlayback,
    ToggleMute,
    /// Open or close the Connect device picker.
    Devices,
    /// Pick a device in the picker: transfer playback there and close it.
    Transfer(String),
    /// Volume slider moved or released; a remote device only hears the final value.
    Volume(f32),
    VolumeReleased,
    /// Progress slider moved or released, with the same remote rule.
    Seek(u64),
    SeekReleased,
    Shortcut(shortcuts::Shortcut),
    Settings,
    Queue,
    QueueScroll(f32, f32, bool),
    QueueMore,
    /// Play a source starting at a known track, shown in the player before Spotify confirms it.
    PlayTrack(PlaySource, Box<Track>),
    /// Like or unlike a song, or save or remove an album or playlist.
    ToggleSaved(library::Entry),
    /// Scrollable moved: report to the glide, then handle as the area's own scroll message.
    Scrolled(smooth_scroll::Area, smooth_scroll::Metrics),
    /// Mouse wheel ticks (in pixels) over a scroll area.
    Wheel(smooth_scroll::Area, f32),
    /// The user grabbed a scroll area; stop gliding.
    GlideStop(smooth_scroll::Area),
    /// Animation frame while a glide is running.
    GlideFrame(std::time::Instant),
    Client(String),
    /// Start browser sign-in, first applying a Client ID that was typed but not saved.
    Authorize,
    Cache(String),
    ThemePath(String),
    ChooseTheme(Option<PathBuf>),
    Save,
    DismissNotice,
    /// A cover finished decoding off the UI thread.
    ArtworkDecoded(String, Result<artwork::Decoded, String>),
    /// Periodic check that dismisses a notice once it has been read.
    NoticeTick(std::time::Instant),
    Tick,
    /// Custom title bar actions.
    Minimize,
    ToggleMaximize,
    CloseWindow,
    DragWindow,
    ResizeWindow(window::Direction),
    /// Whether the window is maximized, queried after it moved or resized.
    Maximized(bool),
    SetZoom(f32),
    SystemTitleBar(bool),
    /// Continue with similar songs after a context ends; saved immediately.
    SetAutoplay(bool),
    /// Show or hide the lyrics of the playing track in the main panel.
    Lyrics,
    /// Animation frame while lyrics are shown during playback.
    LyricsFrame(std::time::Instant),
    LyricsResized(iced::Size),
    LyricsRetry,
    /// Settings draft of the Spicy Lyrics API key, and saving or removing it.
    LyricsKey(String),
    SaveLyricsKey,
    RemoveLyricsKey,
    /// Open an https link from the lyrics credits in the browser.
    OpenLink(String),
    /// A panel boundary was grabbed, the cursor moved to x while dragging it, or released.
    SplitStart(splits::Handle),
    SplitMove(f32),
    SplitEnd,
}

/// A bundled theme offered in Settings; `path` is `None` for the built-in default.
struct ThemeChoice {
    name: String,
    path: Option<PathBuf>,
    palette: Palette,
}

struct Ui {
    pending_preferences: Option<persistence::Preferences>,
    disconnected: bool,
    quit_pending: Option<RequestId>,
    quit_failed: bool,
    active: HashSet<RequestId>,
    artwork: artwork::Artwork,
    /// Layout size of the window (logical size divided by the zoom).
    window_size: iced::Size,
    /// Size and position to open with and to save: the last ones while not maximized.
    restore_size: iced::Size,
    restore_position: Option<iced::Point>,
    /// Latest position reported by the window system; never set on native Wayland.
    position: Option<iced::Point>,
    maximized: bool,
    zoom: f32,
    /// Use the window system's title bar instead of the app's own.
    system_title_bar: bool,
    app_icon: iced::widget::image::Handle,
    sidebar_data: Option<PageData>,
    sidebar_pending: Option<RequestId>,
    sidebar_more: Option<RequestId>,
    sidebar_offset: u64,
    sidebar_filter: String,
    sidebar_scroll: f32,
    sidebar_viewport: f32,
    commands: tokio::sync::mpsc::Sender<Command>,
    state: AppState,
    next: u64,
    window: Option<window::Id>,
    focused: bool,
    palette: Palette,
    glyphs: icons::SolidGlyphs,
    notice: String,
    pending_notices: std::collections::VecDeque<String>,
    /// The notice on screen and when it appeared, for dismissing it automatically.
    notice_shown: Option<(String, std::time::Instant)>,
    /// Background requests (artwork, page loads) whose passing failures are not news.
    quiet: HashSet<RequestId>,
    query: Query,
    title: String,
    /// Header art carried by the current route, such as the artist picture.
    route_artwork: Option<String>,
    /// Home shelves instead of the full playlist grid (see `Route::home`).
    home: bool,
    /// First page of saved albums for the Home shelf, fetched while Home is shown.
    shelf: Option<Page<Album>>,
    shelf_pending: Option<RequestId>,
    /// The shelf request failed; Home shows that instead of retrying on every update.
    shelf_failed: bool,
    /// Discover Weekly and Spotify's other mixes for the Home shelf, read over the
    /// playback session once it is enabled.
    mixes: Option<Vec<Playlist>>,
    mixes_pending: Option<RequestId>,
    /// Why the mixes could not be loaded; Home shows it instead of retrying.
    mixes_failed: Option<String>,
    /// Popular songs of the artist page, fetched beside its albums. When Spotify refuses
    /// them the section is left out instead of showing an error above the discography.
    artist_top: Option<Vec<Track>>,
    artist_top_pending: Option<RequestId>,
    artist_top_failed: bool,
    /// Liked Songs total, known once its page has loaded, for the sidebar subtitle.
    liked_count: Option<u64>,
    /// Which shown songs are liked and which albums and playlists are saved.
    library: library::Library,
    history: History,
    data: Option<PageData>,
    /// A failed main-page load stays visible after its notice expires.
    page_error: Option<String>,
    offset: u64,
    scroll: f32,
    /// Measured size of the main scroll area; estimated until the first layout.
    main_size: iced::Size,
    reset_scroll: bool,
    glide: smooth_scroll::SmoothScroll,
    pending_play: Option<pending_play::PendingPlay>,
    hovered_row: Option<usize>,
    selected_row: Option<usize>,
    track_filter: String,
    track_sort: track_filter::Sort,
    track_menu: Option<Box<Track>>,
    queue_data: Option<PageData>,
    queue_pending: Option<RequestId>,
    queue_more: Option<RequestId>,
    queue_offset: u64,
    queue_scroll: f32,
    queue_viewport: f32,
    pending: Option<RequestId>,
    /// In-flight page that grows or replaces the loaded run of `data`.
    more_pending: Option<(RequestId, paging::Fill)>,
    generation: u64,
    requests: HashMap<RequestId, u64>,
    search: String,
    search_generation: u64,
    settings: bool,
    queue: bool,
    /// Connect device picker is open.
    devices: bool,
    /// Slider values held back from a remote device until the slider is released.
    volume_hold: Option<devices::Held<f32>>,
    seek_hold: Option<devices::Held<u64>>,
    client: String,
    cache: String,
    theme_path: String,
    themes: Vec<ThemeChoice>,
    /// Volume restored when unmuting from the player bar.
    unmuted_volume: f32,
    /// Page whose context this window last started playing, for the header play button.
    play_context: Option<Query>,
    /// Measured, ellipsized labels.
    fits: measure::FitCache,
    /// The right panel shows the playing track's lyrics; exclusive with the queue.
    lyrics_open: bool,
    /// Preferred panel widths and the boundary being dragged.
    splits: splits::Splits,
    split_drag: Option<splits::Drag>,
    lyrics: lyrics::Panel,
    /// Unsaved key typed in Settings; cleared once sent.
    lyrics_key: String,
}
/// Run the native daemon on the main thread. Receivers are consumed once by a persistent task.
pub fn run(port: FrontendPort) -> iced::Result {
    let port: Mutex<Option<FrontendPort>> = Mutex::new(Some(port));
    iced::daemon(
        move || {
            let port: FrontendPort = port
                .lock()
                .expect("boot lock")
                .take()
                .expect("daemon boots once");
            Ui::boot(port)
        },
        Ui::update,
        Ui::view,
    )
    .executor::<executor::SharedExecutor>()
    .title(Ui::window_title)
    .theme(Ui::iced_theme)
    .style(Ui::window_style)
    .scale_factor(Ui::scale_factor)
    .font(icons::FONT_BYTES)
    .subscription(Ui::subscription)
    .run()
}
impl Ui {
    /// Initialize from the restored backend snapshot without triggering a full library refresh.
    fn boot(port: FrontendPort) -> (Self, Task<Message>) {
        let state: AppState = port.state.borrow().clone();
        let mut palette: Palette = Palette::default_theme();
        let mut notice: String = String::new();
        if let Some(path) = &state.settings.theme_path {
            match Palette::load(path) {
                Ok(p) => palette = p,
                Err(e) => notice = e,
            }
        }
        let window_size = iced::Size::new(1180.0, 800.0);
        let mut ui: Self = Self {
            pending_preferences: None,
            disconnected: false,
            quit_pending: None,
            quit_failed: false,
            active: HashSet::new(),
            artwork: Default::default(),
            window_size,
            restore_size: window_size,
            restore_position: None,
            position: None,
            maximized: false,
            zoom: 1.0,
            system_title_bar: false,
            app_icon: {
                let icon = app_icon::rgba(64);
                iced::widget::image::Handle::from_rgba(icon.width, icon.height, icon.pixels)
            },
            sidebar_data: None,
            sidebar_pending: None,
            sidebar_more: None,
            sidebar_offset: 0,
            sidebar_filter: String::new(),
            sidebar_scroll: 0.0,
            sidebar_viewport: window_size.height,
            commands: port.commands,
            client: state.settings.client_id.clone().unwrap_or_default(),
            cache: state.settings.cache_limit_bytes.to_string(),
            theme_path: state
                .settings
                .theme_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            unmuted_volume: if state.player.volume > 0.0 {
                state.player.volume
            } else {
                0.5
            },
            state,
            next: 1,
            window: None,
            focused: true,
            glyphs: icons::SolidGlyphs::new(&palette),
            palette,
            notice,
            pending_notices: std::collections::VecDeque::new(),
            notice_shown: None,
            quiet: HashSet::new(),
            query: Query::OwnPlaylists,
            title: "Home".into(),
            route_artwork: None,
            home: true,
            shelf: None,
            shelf_pending: None,
            shelf_failed: false,
            mixes: None,
            mixes_pending: None,
            mixes_failed: None,
            artist_top: None,
            artist_top_pending: None,
            artist_top_failed: false,
            liked_count: None,
            library: Default::default(),
            history: History::default(),
            data: None,
            page_error: None,
            offset: 0,
            scroll: 0.0,
            main_size: iced::Size::ZERO,
            reset_scroll: false,
            glide: smooth_scroll::SmoothScroll::default(),
            pending_play: None,
            hovered_row: None,
            selected_row: None,
            track_filter: String::new(),
            track_sort: track_filter::Sort::Source,
            track_menu: None,
            queue_data: None,
            queue_pending: None,
            queue_more: None,
            queue_offset: 0,
            queue_scroll: 0.0,
            queue_viewport: window_size.height,
            pending: None,
            more_pending: None,
            generation: 0,
            requests: HashMap::new(),
            search: String::new(),
            search_generation: 0,
            settings: false,
            queue: false,
            devices: false,
            volume_hold: None,
            seek_hold: None,
            themes: Vec::new(),
            play_context: None,
            fits: Default::default(),
            lyrics_open: false,
            splits: Default::default(),
            split_drag: None,
            lyrics: Default::default(),
            lyrics_key: String::new(),
        };
        match persistence::Preferences::load() {
            Ok(p) => {
                if !matches!(ui.state.auth, AuthState::Ready { .. }) {
                    ui.pending_preferences = Some(p.clone());
                }
                p.apply_window(&mut ui);
                p.apply(&mut ui);
            }
            Err(e) => ui.notice = e,
        }
        ui.main_size = ui.estimated_main_size();
        let stream = iced::stream::channel(
            16,
            move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
                let mut events = port.events;
                let mut state = port.state;
                let mut state_open: bool = true;
                loop {
                    tokio::select! {
                        event = events.recv() => match event { Some(e)=>if output.send(Message::Backend(e)).await.is_err(){break}, None=>break },
                        changed = state.changed(), if state_open => { if changed.is_err(){state_open=false;continue} let snapshot: AppState = state.borrow_and_update().clone(); if output.send(Message::State(snapshot)).await.is_err(){break} }
                    }
                }
                let _ = output.send(Message::Disconnected).await;
            },
        );
        let (_, open): (window::Id, Task<window::Id>) = window::open(ui.window_settings());
        (
            ui,
            Task::batch([Task::run(stream, |m| m), open.map(Message::Opened)]),
        )
    }
    /// Send without blocking; expose bounded-channel overload instead of spawning send tasks.
    fn show_notice(&mut self, message: String) {
        if self.notice.is_empty() {
            self.notice = message;
        } else if self.notice != message && !self.pending_notices.contains(&message) {
            self.pending_notices.push_back(message);
        }
    }

    fn send(&mut self, action: Action) -> Option<RequestId> {
        if let Action::CancelRequest(id) = &action {
            self.active.remove(id);
        }
        let cancellation = matches!(action, Action::CancelRequest(_));
        let quitting = matches!(action, Action::Quit);
        let background: bool = is_background(&action);
        let id: RequestId = RequestId(self.next);
        self.next = (self.next + 1).min((1u64 << 63) - 1);
        match self.commands.try_send(Command { id, action }) {
            Ok(()) => {
                if !cancellation {
                    self.active.insert(id);
                }
                if background {
                    self.quiet.insert(id);
                }
                if quitting {
                    self.quit_pending = Some(id);
                }
                Some(id)
            }
            Err(error) => {
                if matches!(error, tokio::sync::mpsc::error::TrySendError::Closed(_)) {
                    self.disconnected = true;
                } else if background {
                    // Background work is requested again as the view settles.
                    return None;
                }
                self.notice = if self.disconnected { "Backend disconnected; checkpoint not confirmed. Use Quit or close to exit without saving." } else { "Backend busy. Please retry." }.into();
                None
            }
        }
    }
    /// Cancel a tracked request, if any.
    fn cancel(&mut self, request: Option<RequestId>) {
        if let Some(id) = request {
            self.send(Action::CancelRequest(id));
        }
    }
    /// Replace the visible page and cancel its predecessor; only matching generation responses apply.
    fn fetch(&mut self) {
        let previous = self.pending.take();
        self.cancel(previous);
        let more = self.more_pending.take().map(|(id, _)| id);
        self.cancel(more);
        if let Err(e) = persistence::Preferences::save(self) {
            self.notice = e;
        }
        self.requests.clear();
        self.generation += 1;
        self.data = None;
        self.page_error = None;
        self.library.retry_failed();
        self.scroll = 0.0;
        self.reset_scroll = true;
        self.hovered_row = None;
        self.selected_row = None;
        self.track_filter.clear();
        self.track_sort = track_filter::Sort::Source;
        self.track_menu = None;
        let artist_top = self.artist_top_pending.take();
        self.cancel(artist_top);
        self.artist_top = None;
        self.artist_top_failed = false;
        if self.home {
            // Refresh the saved-albums shelf with the page; `ensure_shelf` refetches it.
            let shelf = self.shelf_pending.take();
            self.cancel(shelf);
            self.shelf = None;
            self.shelf_failed = false;
            let mixes = self.mixes_pending.take();
            self.cancel(mixes);
            self.mixes = None;
            self.mixes_failed = None;
        }
        // An empty search shows the start page; the backend rejects empty text.
        if matches!(&self.query, Query::Search { text } if text.trim().is_empty()) {
            return;
        }
        if let Some(id) = self.send(Action::Fetch {
            query: self.query.clone(),
            page: PageRequest {
                offset: self.offset,
                limit: MAX_PAGE_SIZE,
            },
        }) {
            self.pending = Some(id);
            self.requests.insert(id, self.generation);
        }
    }
    /// Fetch the page the visible window needs, once at a time. A request the window has
    /// outgrown is cancelled for the one it needs now.
    fn ensure_window(&mut self) {
        if self.home || self.settings || self.pending.is_some() {
            return;
        }
        let Some(data) = &self.data else {
            return;
        };
        if matches!(data, PageData::Tracks(_)) && self.track_view_active() {
            return;
        }
        let wanted: Option<paging::Fill> = paging::wanted_fill(data, self.visible_slots(data));
        if let Some((_, sent)) = &self.more_pending
            && wanted.is_some_and(|w| sent.serves(&w))
        {
            return;
        }
        if let Some((stale, _)) = self.more_pending.take() {
            self.requests.remove(&stale);
            self.cancel(Some(stale));
        }
        let Some(fill) = wanted else {
            return;
        };
        if let Some(id) = self.send(Action::Fetch {
            query: self.query.clone(),
            page: PageRequest {
                offset: fill.offset,
                limit: fill.limit,
            },
        }) {
            self.more_pending = Some((id, fill));
            self.requests.insert(id, self.generation);
        }
    }
    /// Join a fetched page to the loaded run. A page that does not join it replaces the run.
    fn apply_fill(&mut self, kind: paging::FillKind, page: PageData) {
        let rejected: Option<PageData> = match (kind, &mut self.data) {
            (paging::FillKind::Append, Some(run)) => paging::append(run, page).err(),
            (paging::FillKind::Prepend, Some(run)) => paging::prepend(run, page).err(),
            _ => Some(page),
        };
        if let Some(page) = rejected {
            self.data = Some(page);
        }
    }
    /// Fetch the saved-albums shelf while Home is visible and signed in, once per page load.
    fn ensure_shelf(&mut self) {
        if !self.home
            || self.settings
            || self.window.is_none()
            || !matches!(self.state.auth, AuthState::Ready { .. })
            || self.shelf.is_some()
            || self.shelf_pending.is_some()
            || self.shelf_failed
        {
            return;
        }
        self.shelf_pending = self.send(Action::Fetch {
            query: Query::SavedAlbums,
            page: PageRequest {
                offset: 0,
                limit: SHELF_LIMIT,
            },
        });
    }
    /// Fetch the popular songs of the artist page once per page load.
    fn ensure_artist_top(&mut self) {
        let Query::ArtistAlbums { artist_id } = &self.query else {
            return;
        };
        if self.settings
            || self.window.is_none()
            || !matches!(self.state.auth, AuthState::Ready { .. })
            || self.artist_top.is_some()
            || self.artist_top_pending.is_some()
            || self.artist_top_failed
        {
            return;
        }
        self.artist_top_pending = self.send(Action::Fetch {
            query: Query::ArtistTopTracks {
                artist_id: artist_id.clone(),
            },
            page: PageRequest {
                offset: 0,
                limit: SHELF_LIMIT,
            },
        });
    }
    /// Fetch the Made For You shelf while Home is visible, once playback is enabled: the
    /// playback session is what can read Spotify's personalized playlists.
    fn ensure_mixes(&mut self) {
        if !self.home
            || self.settings
            || self.window.is_none()
            || !matches!(self.state.auth, AuthState::Ready { .. })
            || self.state.connect.playback_auth != PlaybackAuth::Ready
            || self.mixes.is_some()
            || self.mixes_pending.is_some()
            || self.mixes_failed.is_some()
        {
            return;
        }
        self.mixes_pending = self.send(Action::Fetch {
            query: Query::MadeForYou,
            page: PageRequest {
                offset: 0,
                limit: SHELF_LIMIT,
            },
        });
    }
    /// Request one revision-tagged queue page without replacing the library browser.
    fn fetch_queue(&mut self) {
        let previous = self.queue_pending.take();
        self.cancel(previous);
        let more = self.queue_more.take();
        self.cancel(more);
        self.queue_data = None;
        self.queue_scroll = 0.0;
        self.queue_pending = self.send(Action::Fetch {
            query: Query::Queue {
                revision: self.state.player.queue_revision,
            },
            page: PageRequest {
                offset: self.queue_offset,
                limit: MAX_PAGE_SIZE,
            },
        });
    }
    /// Append the next queue page for the same revision.
    fn queue_load_more(&mut self) {
        if self.queue_more.is_some() || self.queue_pending.is_some() {
            return;
        }
        let Some(data) = &self.queue_data else {
            return;
        };
        if !paging::can_extend(data) {
            return;
        }
        let offset: u64 = paging::next_offset(data);
        self.queue_more = self.send(Action::Fetch {
            query: Query::Queue {
                revision: self.state.player.queue_revision,
            },
            page: PageRequest {
                offset,
                limit: MAX_PAGE_SIZE,
            },
        });
    }
    /// Queue entries after the current track, which the panel shows separately.
    fn queue_items(&self) -> Vec<&QueueItem> {
        match &self.queue_data {
            Some(PageData::Queue { page, .. }) => page
                .items
                .iter()
                .filter(|item| Some(item.entry.entry_id) != self.state.player.current_entry_id)
                .collect(),
            _ => Vec::new(),
        }
    }
    /// Clear account-scoped state and invalidate every in-flight completion.
    fn clear_account(&mut self) {
        let ids: Vec<_> = self.active.drain().collect();
        for id in ids {
            self.send(Action::CancelRequest(id));
        }
        self.active.clear();
        self.quiet.clear();
        self.pending = None;
        self.more_pending = None;
        self.shelf_pending = None;
        self.shelf = None;
        self.shelf_failed = false;
        self.mixes_pending = None;
        self.mixes = None;
        self.mixes_failed = None;
        self.artist_top_pending = None;
        self.artist_top = None;
        self.artist_top_failed = false;
        self.liked_count = None;
        self.library.clear();
        self.sidebar_pending = None;
        self.sidebar_more = None;
        self.queue_pending = None;
        self.queue_more = None;
        self.requests.clear();
        self.generation += 1;
        self.search_generation += 1;
        self.data = None;
        self.page_error = None;
        self.sidebar_data = None;
        self.queue_data = None;
        self.artwork = Default::default();
        self.state.player.current = None;
        self.state.player.current_entry_id = None;
        self.search.clear();
        self.sidebar_filter.clear();
        self.offset = 0;
        self.sidebar_offset = 0;
        self.queue_offset = 0;
        self.scroll = 0.0;
        self.sidebar_scroll = 0.0;
        self.queue_scroll = 0.0;
        self.queue = false;
        self.history = History::default();
        let home: Route = Route::home();
        self.query = home.query;
        self.title = home.title;
        self.route_artwork = None;
        self.home = true;
        if let Err(e) = persistence::Preferences::save(self) {
            self.notice = e;
        }
    }
    /// Swap the theme; covers are re-decoded when their baked corner radius changes.
    fn set_palette(&mut self, palette: Palette) {
        if palette == self.palette {
            return;
        }
        let rounding_changed: bool = palette.shape.radius_small != self.palette.shape.radius_small;
        self.palette = palette;
        self.glyphs = icons::SolidGlyphs::new(&palette);
        if rounding_changed {
            self.reset_artwork();
        }
    }
    /// Apply changed authoritative configuration, not unrelated playback snapshots.
    fn sync_settings(&mut self, settings: &crate::domain::Settings) {
        let old = &self.state.settings;
        if old.client_id == settings.client_id
            && old.cache_limit_bytes == settings.cache_limit_bytes
            && old.theme_path == settings.theme_path
            && old.autoplay == settings.autoplay
        {
            return;
        }
        self.client = settings.client_id.clone().unwrap_or_default();
        self.cache = settings.cache_limit_bytes.to_string();
        self.theme_path = settings
            .theme_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        match settings
            .theme_path
            .as_ref()
            .map(|p| Palette::load(p))
            .unwrap_or_else(|| Ok(Palette::default_theme()))
        {
            Ok(p) => self.set_palette(p),
            Err(e) => self.notice = e,
        }
    }
    /// Reset Settings drafts from authoritative state and list the bundled themes.
    fn open_settings(&mut self) {
        self.client = self.state.settings.client_id.clone().unwrap_or_default();
        self.cache = self.state.settings.cache_limit_bytes.to_string();
        self.theme_path = self
            .state
            .settings
            .theme_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.themes = bundled_themes();
    }
    /// Sign-in needs a saved Client ID. A typed one is saved first (the backend handles
    /// commands in order); with none at all, Settings opens to ask for it instead.
    fn authorize(&mut self) {
        let typed: &str = self.client.trim();
        if !typed.is_empty() && self.state.settings.client_id.as_deref() != Some(typed) {
            let mut settings: crate::domain::Settings = self.state.settings.clone();
            settings.client_id = Some(typed.into());
            self.send(Action::Configure(settings));
        } else if typed.is_empty() && self.state.settings.client_id.is_none() {
            self.show_notice(
                "Add your Spotify developer app's Client ID in Settings, then authorize.".into(),
            );
            if !self.settings {
                let mut route: Route = self.route();
                route.settings = true;
                self.navigate(route);
            }
            return;
        }
        self.send(Action::BeginLogin);
    }
    /// The place the main view currently shows.
    fn route(&self) -> Route {
        Route {
            query: self.query.clone(),
            title: self.title.clone(),
            artwork: self.route_artwork.clone(),
            settings: self.settings,
            home: self.home,
        }
    }
    /// Open a new place and remember the current one for Back.
    fn navigate(&mut self, route: Route) {
        let current: Route = self.route();
        if current == route {
            return;
        }
        self.history.push(current);
        self.go(route);
    }
    /// Playback position now: the anchor extrapolated while playing, clamped to the track.
    fn position_ms(&self) -> u64 {
        let player: &PlayerState = &self.state.player;
        let duration: u64 = player.current.as_ref().map_or(0, |t| t.duration_ms);
        let elapsed: u64 = if player.status == PlaybackStatus::Playing {
            player
                .observed_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
        } else {
            0
        };
        (player.position_ms + elapsed).min(duration)
    }
    /// Request the playing track's lyrics while the lyrics view is open.
    fn sync_lyrics(&mut self) -> Task<Message> {
        if !self.lyrics_open {
            return Task::none();
        }
        let track: Option<SpotifyId> = self.state.player.current.as_ref().map(|t| t.id.clone());
        // Waiting for a key, or a send the busy backend refused: try again once possible.
        let unasked: bool = self.lyrics.status.is_none() && self.state.lyrics_key_set;
        if track != self.lyrics.track || unasked {
            if let Some(id) = self.lyrics.loading() {
                self.send(Action::CancelRequest(id));
            }
            let status: Option<lyrics::Status> = match &track {
                Some(track_id) if self.state.lyrics_key_set => self
                    .send(Action::Lyrics {
                        track_id: track_id.clone(),
                    })
                    .map(lyrics::Status::Loading),
                _ => None,
            };
            self.lyrics.reset(track, status);
            if self.window.is_some() {
                self.glide.cancel(smooth_scroll::Area::Lyrics);
                return iced::widget::operation::snap_to(
                    smooth_scroll::Area::Lyrics.id(),
                    iced::widget::operation::RelativeOffset::START,
                );
            }
        }
        self.follow_lyrics();
        Task::none()
    }
    /// Glide the lyrics view to the sung line when it changed.
    fn follow_lyrics(&mut self) {
        if !self.lyrics_open {
            return;
        }
        let font: iced::Font = self.palette.bold_font();
        let position: u64 = self.position_ms();
        if let Some(y) = self
            .lyrics
            .follow(position, std::time::Instant::now(), |l, size| {
                lyrics::tops(&l.lines, size, font)
            })
        {
            self.glide.ease_to(smooth_scroll::Area::Lyrics, y);
        }
    }
    /// Show a route; the page is only refetched when its query changes.
    fn go(&mut self, route: Route) {
        if route.settings && !self.settings {
            self.open_settings();
        }
        let changed: bool = route.query != self.query;
        if route.settings != self.settings || route.home != self.home {
            self.scroll = 0.0;
            self.reset_scroll = true;
        }
        self.query = route.query;
        self.title = route.title;
        self.route_artwork = route.artwork;
        self.settings = route.settings;
        self.home = route.home;
        if changed || (self.data.is_none() && self.pending.is_none()) {
            self.offset = 0;
            self.fetch();
        } else if let Err(e) = persistence::Preferences::save(self) {
            self.notice = e;
        }
    }
    /// Reduce one message, then reconcile artwork and pending scroll resets.
    fn update(&mut self, message: Message) -> Task<Message> {
        // Animation frames arrive many times a second and change no data, so they skip the
        // reconciliation below; the scroll events a glide causes still run it.
        if matches!(message, Message::LyricsFrame(_) | Message::GlideFrame(_)) {
            return self.reduce(message);
        }
        let task: Task<Message> = self.reduce(message);
        self.ensure_shelf();
        self.ensure_mixes();
        self.ensure_artist_top();
        self.fetch_artwork();
        self.check_library();
        if self.notice.is_empty() {
            self.notice_shown = None;
        } else if self
            .notice_shown
            .as_ref()
            .is_none_or(|(shown, _)| *shown != self.notice)
        {
            self.notice_shown = Some((self.notice.clone(), std::time::Instant::now()));
        }
        if std::mem::take(&mut self.reset_scroll) && self.window.is_some() {
            self.glide.cancel(smooth_scroll::Area::Main);
            let top = iced::widget::operation::scroll_to(
                iced::widget::Id::new(MAIN_SCROLL),
                iced::widget::operation::AbsoluteOffset {
                    x: 0.0,
                    y: self.scroll,
                },
            );
            return Task::batch([task, top]);
        }
        task
    }
    /// Reduce events; failed shutdown permits an explicit, warned uncheckpointed exit.
    fn reduce(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Opened(id) => {
                self.window = Some(id);
                self.focused = true;
                self.send(Action::SetVisible(true));
                self.fetch_sidebar();
                if self.data.is_none() {
                    self.fetch();
                }
                if self.queue
                    && !matches!(&self.queue_data, Some(PageData::Queue { revision, .. }) if *revision == self.state.player.queue_revision)
                {
                    self.fetch_queue();
                }
                return self.ensure_on_screen(id);
            }
            Message::Window(_, window::Event::CloseRequested) => {
                if self.disconnected || self.quit_failed {
                    return iced::exit();
                }
                // A plain window app: closing quits through the normal checkpointed shutdown.
                return self.reduce(Message::Action(Action::Quit));
            }
            Message::Window(id, window::Event::Resized(size)) => {
                self.window_size = size;
                // Estimate until the scrollables report exact bounds.
                self.main_size = self.estimated_main_size();
                self.sidebar_viewport = size.height;
                self.queue_viewport = size.height;
                return self.track_maximized(id);
            }
            Message::Window(_, window::Event::Moved(position)) => self.set_position(position),
            Message::Maximized(maximized) => self.set_maximized(maximized),
            Message::Minimize => {
                if let Some(id) = self.window {
                    return window::minimize(id, true);
                }
            }
            Message::ToggleMaximize => {
                if let Some(id) = self.window {
                    // The resize that follows records the new state and restore geometry.
                    if self.tracks_own_maximize() {
                        self.maximized = !self.maximized;
                    }
                    return window::toggle_maximize(id);
                }
            }
            Message::DragWindow => {
                if let Some(id) = self.window {
                    return window::drag(id);
                }
            }
            Message::ResizeWindow(direction) => {
                if let Some(id) = self.window {
                    return window::drag_resize(id, direction);
                }
            }
            // Exactly the window manager's close request, which quits.
            Message::CloseWindow => {
                if let Some(id) = self.window {
                    return self.reduce(Message::Window(id, window::Event::CloseRequested));
                }
            }
            Message::SetZoom(zoom) => return self.set_zoom(zoom),
            Message::SystemTitleBar(system) => return self.set_system_title_bar(system),
            Message::Window(_, window::Event::Focused) => {
                self.focused = true;
            }
            Message::Window(_, window::Event::Unfocused) => {
                self.focused = false;
            }
            Message::Window(_, _) | Message::Tick => {}
            Message::Disconnected => {
                self.disconnected = true;
                self.notice = "Backend disconnected; checkpoint not confirmed. Use Quit or close to exit without saving.".into()
            }
            Message::State(state) => {
                self.sync_settings(&state.settings);
                let previous_user = match &self.state.auth {
                    AuthState::Ready { user_id } => Some(user_id.clone()),
                    _ => None,
                };
                let next_user = match &state.auth {
                    AuthState::Ready { user_id } => Some(user_id.clone()),
                    _ => None,
                };
                let identity_changed = previous_user.is_some() && previous_user != next_user;
                if identity_changed {
                    self.clear_account();
                }
                let newly_ready = !matches!(self.state.auth, AuthState::Ready { .. })
                    && matches!(state.auth, AuthState::Ready { .. });
                let revision_changed: bool =
                    self.state.player.queue_revision != state.player.queue_revision;
                let was_remote: bool = self.state.connect.is_remote();
                if state.player.volume > 0.0 {
                    self.unmuted_volume = state.player.volume;
                }
                self.state = state;
                if let Some(pending) = self.pending_play.as_mut() {
                    if pending.observe(&self.state.player) {
                        pending.show(&mut self.state.player);
                    } else {
                        self.pending_play = None;
                    }
                }
                let remote: bool = self.state.connect.is_remote();
                self.volume_hold = self.volume_hold.filter(|h| h.shown().is_some());
                // A released seek shows until the player reaches it or the settle time ends.
                let position: u64 = self.position_ms();
                self.seek_hold = self.seek_hold.filter(|h| {
                    h.shown().is_some()
                        && (h.is_dragging() || h.value.abs_diff(position) > SEEK_REACHED_MS)
                });
                if newly_ready && let Some(preferences) = self.pending_preferences.take() {
                    preferences.apply(self);
                }
                if (newly_ready || identity_changed)
                    && matches!(self.state.auth, AuthState::Ready { .. })
                    && self.window.is_some()
                {
                    self.fetch_sidebar();
                    self.fetch();
                }
                // The queue panel lists the local queue; back from a remote device it is stale.
                if self.window.is_some()
                    && self.queue
                    && !remote
                    && (revision_changed || was_remote)
                {
                    self.queue_offset = 0;
                    self.fetch_queue();
                }
                // After restored preferences, which may reopen the lyrics view.
                return self.sync_lyrics();
            }
            Message::Backend(Event::QuitReady) => return iced::exit(),
            Message::Backend(Event::ShowWindow) => {
                if let Some(id) = self.window {
                    return window::gain_focus(id);
                }
                let (id, task) = window::open(self.window_settings());
                self.window = Some(id);
                return task.map(Message::Opened);
            }
            Message::Backend(Event::Notice(error)) => {
                if self.quit_pending.is_some() {
                    self.quit_failed = true;
                    self.notice = format!(
                        "{} Checkpoint not confirmed. Use Quit or close again to exit without saving.",
                        error.message
                    );
                } else {
                    self.show_notice(error.message);
                }
            }
            Message::Backend(Event::Completed { id, result }) => {
                if !self.active.remove(&id) {
                    return Task::none();
                }
                // Lyrics outcomes, failures included, are shown in the lyrics view.
                if self.lyrics.loading() == Some(id) {
                    self.quiet.remove(&id);
                    self.lyrics.complete(match result {
                        Ok(Response::Lyrics(Some(found))) => lyrics::Status::Ready(found),
                        Ok(_) => lyrics::Status::Missing,
                        Err(e) => lyrics::Status::Failed(e),
                    });
                    self.follow_lyrics();
                    return Task::none();
                }
                if self.complete_library(id, &result) {
                    return Task::none();
                }
                let quiet: bool = self.quiet.remove(&id);
                if result.is_err()
                    && let Some(pending) =
                        self.pending_play.take_if(|pending| pending.request == id)
                {
                    pending.restore(&mut self.state.player);
                }
                match result {
                    Ok(Response::Artwork { bytes }) => {
                        return self
                            .complete_artwork(id, Some(bytes))
                            .unwrap_or_else(Task::none);
                    }
                    Ok(Response::Page(data)) => self.complete_page(id, data),
                    Err(e) => {
                        let artwork: bool = self.complete_artwork(id, None).is_some();
                        // Requests sent while startup sign-in is still running are retried
                        // once the backend reports Ready, so their failure isn't news.
                        let early: bool = e.kind == ErrorKind::Authentication
                            && matches!(self.state.auth, AuthState::Restoring);
                        if self.pending == Some(id) && !early {
                            self.page_error = Some(e.message.clone());
                        }
                        if self.shelf_pending == Some(id) && !early {
                            self.shelf_failed = true;
                        }
                        if self.mixes_pending == Some(id) && !early {
                            self.mixes_failed = Some(e.message.clone());
                        }
                        // The section is left out when refused, so that is no notice.
                        let omitted: bool = self.artist_top_pending == Some(id);
                        if omitted && !early {
                            self.artist_top_failed = true;
                        }
                        for slot in [
                            &mut self.artist_top_pending,
                            &mut self.shelf_pending,
                            &mut self.mixes_pending,
                            &mut self.sidebar_pending,
                            &mut self.sidebar_more,
                            &mut self.pending,
                            &mut self.queue_pending,
                            &mut self.queue_more,
                        ] {
                            if *slot == Some(id) {
                                *slot = None;
                            }
                        }
                        if self.more_pending.is_some_and(|(more, _)| more == id) {
                            self.more_pending = None;
                        }
                        self.requests.remove(&id);
                        if self.quit_pending == Some(id) {
                            self.quit_failed = true;
                            self.notice = format!(
                                "{} Checkpoint not confirmed. Use Quit or close again to exit without saving.",
                                e.message
                            );
                        } else if !artwork && !early && !omitted && !(quiet && passing(e.kind)) {
                            self.show_notice(e.message);
                        }
                    }
                    _ => {}
                }
            }
            Message::TogglePlayback => {
                self.send(if self.state.player.status == PlaybackStatus::Playing {
                    Action::Pause
                } else {
                    Action::Resume
                });
            }
            Message::ToggleMute => {
                let volume: f32 = if self.state.player.volume > 0.0 {
                    0.0
                } else {
                    self.unmuted_volume
                };
                self.send(Action::SetVolume(volume));
            }
            Message::Devices => {
                self.devices = !self.devices;
                if self.devices {
                    self.send(Action::RefreshDevices);
                }
            }
            Message::Transfer(device_id) => {
                self.devices = false;
                self.send(Action::TransferPlayback {
                    device_id,
                    play: true,
                });
            }
            Message::Volume(volume) => {
                if self.state.connect.is_remote() {
                    self.volume_hold = Some(devices::Held::dragging(volume));
                } else {
                    self.send(Action::SetVolume(volume));
                }
            }
            Message::VolumeReleased => {
                if let Some(held) = self.volume_hold.as_mut().filter(|h| h.is_dragging()) {
                    held.release();
                    let volume: f32 = held.value;
                    self.send(Action::SetVolume(volume));
                }
            }
            // Every step of a drag would be a decoder seek, so only the release seeks.
            Message::Seek(position_ms) => {
                self.seek_hold = Some(devices::Held::dragging(position_ms));
            }
            Message::SeekReleased => {
                if let Some(held) = self.seek_hold.as_mut().filter(|h| h.is_dragging()) {
                    held.release();
                    let position_ms: u64 = held.value;
                    self.send(Action::Seek { position_ms });
                }
            }
            Message::Shortcut(shortcut) => return self.shortcut(shortcut),
            Message::Action(action) => {
                self.track_menu = None;
                if matches!(action, Action::Quit) && (self.disconnected || self.quit_failed) {
                    return iced::exit();
                }
                if let Action::Play(source) = &action {
                    self.play_context = context_query(source);
                }
                if matches!(action, Action::Logout) {
                    self.clear_account();
                }
                self.send(action);
            }
            Message::PlayTrack(source, track) => {
                self.track_menu = None;
                self.play_context = context_query(&source);
                if let Some(request) = self.send(Action::Play(source)) {
                    let pending =
                        pending_play::PendingPlay::new(*track, request, &self.state.player);
                    pending.show(&mut self.state.player);
                    self.pending_play = Some(pending);
                }
            }
            Message::SidebarFilter(value) => {
                self.sidebar_filter = value;
                self.sidebar_scroll = 0.0;
                self.glide.cancel(smooth_scroll::Area::Sidebar);
            }
            Message::SidebarScroll(y, height, near_end) => {
                self.sidebar_scroll = y;
                self.sidebar_viewport = height;
                if near_end {
                    self.sidebar_load_more();
                }
            }
            Message::SidebarMore => self.sidebar_load_more(),
            Message::Search(value) => {
                self.search = value;
                self.search_generation = self.search_generation.wrapping_add(1);
                let generation: u64 = self.search_generation;
                return Task::perform(
                    async move {
                        tokio::time::sleep(Duration::from_millis(350)).await;
                        generation
                    },
                    Message::SearchDebounced,
                );
            }
            Message::SearchDebounced(generation) => {
                if generation == self.search_generation && self.window.is_some() {
                    return self.reduce(Message::SearchNow);
                }
            }
            Message::SearchNow => {
                let text: String = self.search.trim().into();
                let searching: bool = is_search(&self.query) && !self.settings;
                // Clearing the field only resets an open search page to its start page.
                if !text.is_empty() || searching {
                    let route: Route = Route::search(&text);
                    // Refining a search replaces it rather than filling the history.
                    if searching {
                        self.go(route);
                    } else {
                        self.navigate(route);
                    }
                }
            }
            Message::FocusSearch => {
                return iced::widget::operation::focus(iced::widget::Id::new(SEARCH_INPUT));
            }
            Message::OpenSearch => {
                if !is_search(&self.query) || self.settings {
                    self.navigate(Route::search(self.search.trim()));
                }
                return iced::widget::operation::focus(iced::widget::Id::new(SEARCH_INPUT));
            }
            Message::Navigate(route) => {
                self.track_menu = None;
                self.navigate(route);
            }
            Message::ToggleSaved(entry) => {
                self.track_menu = None;
                self.toggle_saved(entry);
            }
            Message::RetryPage => self.fetch(),
            Message::Back => {
                if let Some(route) = self.history.back(self.route()) {
                    self.go(route);
                    self.show_search_text();
                }
            }
            Message::Forward => {
                if let Some(route) = self.history.forward(self.route()) {
                    self.go(route);
                    self.show_search_text();
                }
            }
            Message::Settings => {
                let mut route: Route = self.route();
                route.settings = !route.settings;
                self.navigate(route);
            }
            Message::Scroll(y, height) => {
                self.scroll = y;
                self.main_size.height = height;
                self.ensure_window();
            }
            Message::MainResized(size) => {
                self.main_size = size;
                self.ensure_window();
            }
            Message::HoverRow(index, entered) => {
                if entered {
                    self.hovered_row = Some(index);
                } else if self.hovered_row == Some(index) {
                    self.hovered_row = None;
                }
            }
            Message::SelectRow(index) => self.selected_row = Some(index),
            Message::TrackFilter(filter) => {
                self.track_filter = filter;
                self.selected_row = None;
                self.scroll = 0.0;
                self.reset_scroll = true;
            }
            Message::TrackSort(sort) => {
                self.track_sort = sort;
                self.selected_row = None;
                self.scroll = 0.0;
                self.reset_scroll = true;
            }
            Message::TrackMenu(track) => self.track_menu = Some(track),
            Message::CloseTrackMenu => self.track_menu = None,
            Message::CopyLink(link) => {
                self.track_menu = None;
                return iced::clipboard::write(link);
            }
            Message::RetryShelves => {
                self.shelf_failed = false;
                self.mixes_failed = None;
            }
            Message::TrackMore => {
                if self.more_pending.is_none()
                    && let Some(data) = &self.data
                    && paging::can_extend(data)
                {
                    let fill = paging::Fill {
                        kind: paging::FillKind::Append,
                        offset: paging::next_offset(data),
                        limit: MAX_PAGE_SIZE,
                    };
                    if let Some(id) = self.send(Action::Fetch {
                        query: self.query.clone(),
                        page: PageRequest {
                            offset: fill.offset,
                            limit: fill.limit,
                        },
                    }) {
                        self.more_pending = Some((id, fill));
                        self.requests.insert(id, self.generation);
                    }
                }
            }
            Message::RowMove(down) => {
                if self.settings || self.home || self.track_menu.is_some() {
                    return Task::none();
                }
                if let Some(PageData::Tracks(page)) = &self.data {
                    let indices =
                        track_filter::indices(&page.items, &self.track_filter, self.track_sort);
                    let position = self
                        .selected_row
                        .and_then(|i| indices.iter().position(|&n| n + page.offset as usize == i));
                    let next = position.map_or(0, |p| {
                        if down {
                            (p + 1).min(indices.len().saturating_sub(1))
                        } else {
                            p.saturating_sub(1)
                        }
                    });
                    self.selected_row = indices.get(next).map(|&i| i + page.offset as usize);
                    if self.selected_row.is_some() {
                        self.scroll = (self.content_top(self.data.as_ref().unwrap())
                            + (if self.track_view_active() {
                                next
                            } else {
                                page.offset as usize + next
                            }) as f32
                                * self.palette.shape.row_height
                            - self.main_size.height / 2.0)
                            .max(0.0);
                        self.reset_scroll = true;
                    }
                }
            }
            Message::PlaySelected => {
                if self.settings || self.home || self.track_menu.is_some() {
                    return Task::none();
                }
                if let Some(PageData::Tracks(page)) = &self.data
                    && let Some(index) = self
                        .selected_row
                        .and_then(|i| i.checked_sub(page.offset as usize))
                    && let Some(track) = page.items.get(index)
                    && track.playable
                {
                    return self.reduce(Message::PlayTrack(
                        self.source(&page.items, page.offset, index),
                        Box::new(track.clone()),
                    ));
                }
            }
            Message::Queue => {
                self.queue = !self.queue;
                self.lyrics_open &= !self.queue;
                self.main_size = self.estimated_main_size();
                // A remote device shows the preview the backend already holds.
                if self.queue && !self.state.connect.is_remote() {
                    self.queue_offset = 0;
                    self.fetch_queue();
                }
                if let Err(e) = persistence::Preferences::save(self) {
                    self.notice = e;
                }
            }
            Message::QueueScroll(y, height, near_end) => {
                self.queue_scroll = y;
                self.queue_viewport = height;
                if near_end && !self.state.connect.is_remote() {
                    self.queue_load_more();
                }
            }
            Message::QueueMore => self.queue_load_more(),
            Message::Scrolled(area, m) => {
                self.glide.observe(area, m);
                let next = match area {
                    smooth_scroll::Area::Lyrics => {
                        let animating: bool = self.glide.animating(area);
                        self.lyrics
                            .observe(m.y, m.max, animating, std::time::Instant::now());
                        return Task::none();
                    }
                    smooth_scroll::Area::Main => Message::Scroll(m.y, m.height),
                    smooth_scroll::Area::Sidebar => {
                        Message::SidebarScroll(m.y, m.height, m.near_end())
                    }
                    smooth_scroll::Area::Queue => Message::QueueScroll(m.y, m.height, m.near_end()),
                };
                return self.reduce(next);
            }
            Message::Wheel(area, delta) => {
                let now = std::time::Instant::now();
                if area == smooth_scroll::Area::Lyrics {
                    self.lyrics.manual(now);
                }
                self.glide.wheel(area, delta, now);
            }
            Message::GlideStop(area) => {
                if area == smooth_scroll::Area::Lyrics {
                    self.lyrics.manual(std::time::Instant::now());
                }
                self.glide.cancel(area);
            }
            Message::GlideFrame(now) => return self.glide.frame(now),
            Message::Lyrics => {
                self.lyrics_open = !self.lyrics_open;
                // One right panel at a time, as in Spotify.
                self.queue &= !self.lyrics_open;
                self.main_size = self.estimated_main_size();
                if let Err(e) = persistence::Preferences::save(self) {
                    self.notice = e;
                }
                return self.sync_lyrics();
            }
            Message::LyricsFrame(now) => {
                self.follow_lyrics();
                return self.glide.frame(now);
            }
            Message::LyricsResized(size) => {
                self.lyrics.viewport = Some(size);
                self.follow_lyrics();
            }
            Message::LyricsRetry => {
                self.lyrics.reset(None, None);
                return self.sync_lyrics();
            }
            Message::LyricsKey(key) => self.lyrics_key = key,
            Message::SaveLyricsKey => {
                let key: String = self.lyrics_key.trim().to_owned();
                if !key.is_empty() && self.send(Action::SetLyricsKey(Some(key))).is_some() {
                    self.lyrics_key.clear();
                    // Fetch again with the new key.
                    self.lyrics.reset(None, None);
                }
            }
            Message::RemoveLyricsKey => {
                self.send(Action::SetLyricsKey(None));
            }
            Message::SplitStart(handle) => {
                self.split_drag = Some(splits::Drag { handle, grab: None });
            }
            Message::SplitMove(x) => {
                if let Some(mut drag) = self.split_drag {
                    let (inner, right) = (self.frame_inner(), self.right_panel());
                    let shown: f32 = self.splits.shown(drag.handle, inner, right);
                    let width: f32 = drag.width(x, shown);
                    self.splits.set(drag.handle, width, inner, right);
                    self.split_drag = Some(drag);
                    self.main_size = self.estimated_main_size();
                    self.ensure_window();
                }
            }
            Message::SplitEnd => {
                if self.split_drag.take().is_some()
                    && let Err(e) = persistence::Preferences::save(self)
                {
                    self.notice = e;
                }
            }
            Message::OpenLink(url) => {
                if url.starts_with("https://")
                    && let Err(e) = open::that_detached(&url)
                {
                    self.show_notice(format!("Could not open the link: {e}"));
                }
            }
            Message::SetAutoplay(enabled) => {
                let mut settings: crate::domain::Settings = self.state.settings.clone();
                settings.autoplay = enabled;
                self.send(Action::Configure(settings));
            }
            Message::Client(s) => self.client = s,
            Message::Authorize => self.authorize(),
            Message::Cache(s) => self.cache = s,
            Message::ThemePath(s) => self.theme_path = s,
            Message::ChooseTheme(path) => {
                let palette = match &path {
                    Some(path) => Palette::load(path),
                    None => Ok(Palette::default_theme()),
                };
                match palette {
                    Ok(p) => {
                        self.set_palette(p);
                        self.theme_path = path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        let mut settings: crate::domain::Settings = self.state.settings.clone();
                        settings.theme_path = path;
                        self.send(Action::Configure(settings));
                    }
                    Err(e) => self.notice = e,
                }
            }
            Message::Save => match self.cache.parse::<u64>() {
                Ok(limit) => {
                    let path = self.theme_path.trim();
                    let palette = if path.is_empty() {
                        Ok(Palette::default_theme())
                    } else {
                        Palette::load(std::path::Path::new(path))
                    };
                    match palette {
                        Ok(p) => {
                            let mut settings: crate::domain::Settings = self.state.settings.clone();
                            settings.client_id = if self.client.trim().is_empty() {
                                None
                            } else {
                                Some(self.client.trim().into())
                            };
                            settings.cache_limit_bytes = limit;
                            settings.theme_path = if path.is_empty() {
                                None
                            } else {
                                Some(path.into())
                            };
                            self.set_palette(p);
                            self.send(Action::Configure(settings));
                        }
                        Err(e) => self.notice = e,
                    }
                }
                Err(_) => self.notice = "Cache budget must be an integer number of bytes.".into(),
            },
            Message::DismissNotice => {
                self.notice = self.pending_notices.pop_front().unwrap_or_default()
            }
            Message::ArtworkDecoded(url, decoded) => self.artwork_decoded(url, decoded),
            Message::NoticeTick(now) => {
                if self
                    .notice_shown
                    .as_ref()
                    .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= NOTICE_DURATION)
                {
                    self.notice = self.pending_notices.pop_front().unwrap_or_default()
                }
            }
        }
        Task::none()
    }
    /// Put the text of a search page reached through history back into the search field.
    fn show_search_text(&mut self) {
        if let Query::Search { text }
        | Query::SearchArtists { text }
        | Query::SearchAlbums { text }
        | Query::SearchTracks { text }
        | Query::SearchPlaylists { text } = &self.query
        {
            self.search = text.clone();
        }
    }
    /// Route a page completion to the sidebar, shelf, queue or browser it belongs to.
    fn complete_page(&mut self, id: RequestId, data: PageData) {
        if self.artist_top_pending == Some(id) {
            self.artist_top_pending = None;
            match data {
                PageData::Tracks(page) => self.artist_top = Some(page.items),
                _ => self.artist_top_failed = true,
            }
        } else if self.mixes_pending == Some(id) {
            self.mixes_pending = None;
            match data {
                PageData::Playlists(page) => self.mixes = Some(page.items),
                _ => self.mixes_failed = Some("Unexpected response".into()),
            }
        } else if self.shelf_pending == Some(id) {
            self.shelf_pending = None;
            match data {
                PageData::Albums(page) => {
                    self.library.mark(
                        page.items.iter().map(|a| LibraryItem::Album(a.id.clone())),
                        true,
                    );
                    self.shelf = Some(page);
                }
                _ => self.shelf_failed = true,
            }
        } else if self.sidebar_pending == Some(id) {
            self.sidebar_pending = None;
            if matches!(data, PageData::Playlists(_)) {
                self.sidebar_data = Some(data);
            }
        } else if self.sidebar_more == Some(id) {
            self.sidebar_more = None;
            if let Some(existing) = &mut self.sidebar_data {
                let _ = paging::append(existing, data);
            }
        } else if self.queue_pending == Some(id) {
            self.queue_pending = None;
            if let PageData::Queue { revision, .. } = &data
                && *revision == self.state.player.queue_revision
            {
                self.queue_data = Some(data);
            }
        } else if self.queue_more == Some(id) {
            self.queue_more = None;
            if let Some(existing) = &mut self.queue_data {
                let _ = paging::append(existing, data);
            }
        } else if self.requests.remove(&id) == Some(self.generation) {
            if let PageData::Queue { revision, .. } = &data
                && *revision != self.state.player.queue_revision
            {
                return;
            }
            match (&self.query, &data) {
                (Query::LikedTracks, PageData::Tracks(page)) => {
                    self.liked_count = page.total.or(self.liked_count);
                    self.library.mark(
                        page.items.iter().map(|t| LibraryItem::Track(t.id.clone())),
                        true,
                    );
                }
                (Query::SavedAlbums, PageData::Albums(page)) => self.library.mark(
                    page.items.iter().map(|a| LibraryItem::Album(a.id.clone())),
                    true,
                ),
                _ => {}
            }
            if let Some((more, fill)) = self.more_pending
                && more == id
            {
                self.more_pending = None;
                if fill.kind != paging::FillKind::Append {
                    self.selected_row = None;
                    self.hovered_row = None;
                }
                self.apply_fill(fill.kind, data);
            } else {
                self.pending = None;
                self.data = Some(data);
            }
            self.ensure_window();
        }
    }
    /// Subscribe to native events and focused visible playback only; never tick while hidden.
    fn subscription(&self) -> Subscription<Message> {
        let events: Subscription<Message> = Subscription::batch([
            window::events().map(|(id, event)| Message::Window(id, event)),
            iced::event::listen_with(shortcut),
        ]);
        let events: Subscription<Message> = if self.split_drag.is_some() {
            Subscription::batch([events, iced::event::listen_with(split_drag)])
        } else {
            events
        };
        // Disconnect and failed-quit notices explain how to exit, so they stay until acted on.
        let events: Subscription<Message> =
            if self.notice_shown.is_some() && !self.disconnected && !self.quit_failed {
                Subscription::batch([
                    events,
                    iced::time::every(Duration::from_secs(1)).map(Message::NoticeTick),
                ])
            } else {
                events
            };
        let playing: bool = self.state.player.status == PlaybackStatus::Playing;
        // The lyrics frame also drives the glide, so only one of them listens to frames.
        let events: Subscription<Message> = if self.lyrics_open
            && self.window.is_some()
            && self.focused
            && playing
            && self.lyrics.animated()
        {
            Subscription::batch([
                events,
                iced::time::every(LYRICS_FRAME).map(Message::LyricsFrame),
            ])
        } else if self.glide.active() {
            Subscription::batch([events, window::frames().map(Message::GlideFrame)])
        } else {
            events
        };
        if self.window.is_some()
            && self.focused
            && self.state.player.status == PlaybackStatus::Playing
        {
            Subscription::batch([
                events,
                iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
            ])
        } else {
            events
        }
    }
    /// UI zoom: layout is drawn at this scale inside the same window.
    fn scale_factor(&self, _: window::Id) -> f32 {
        self.zoom
    }

    /// Apply declarative colors without changing widget hierarchy or layout.
    fn iced_theme(&self, _: window::Id) -> Theme {
        self.palette.iced_theme()
    }
    /// Leave the area behind the frame clear, so its rounded corners show the desktop. The
    /// frame paints the opaque background itself.
    fn window_style(&self, theme: &Theme) -> iced::theme::Style {
        iced::theme::Style {
            background_color: iced::Color::TRANSPARENT,
            text_color: theme.palette().text,
        }
    }
    /// Name the window after the current page.
    fn window_title(&self, _: window::Id) -> String {
        if self.settings {
            "Scytify — Settings".into()
        } else {
            format!("Scytify — {}", self.title)
        }
    }
}

/// Bundled themes next to the sources, plus the built-in default. Bounded and validated.
fn bundled_themes() -> Vec<ThemeChoice> {
    let mut themes: Vec<ThemeChoice> = vec![ThemeChoice {
        name: "Spotify dark".into(),
        path: None,
        palette: Palette::default_theme(),
    }];
    let dir: &std::path::Path =
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/themes"));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return themes;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter(|p| p.file_stem().is_some_and(|s| s != "spotify-dark"))
        .take(16)
        .collect();
    paths.sort();
    for path in paths {
        if let Ok(palette) = Palette::load(&path) {
            let stem: String = path
                .file_stem()
                .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
                .unwrap_or_default();
            let mut name: Vec<char> = stem.chars().collect();
            if let Some(first) = name.first_mut() {
                *first = first.to_ascii_uppercase();
            }
            themes.push(ThemeChoice {
                name: name.into_iter().collect(),
                path: Some(path),
                palette,
            });
        }
    }
    themes
}

impl Ui {
    /// Main scroll area estimated from the window until a layout reports it. It mirrors
    /// the frame in `view` exactly: when a resize event arrives after the first layout,
    /// the measured size does not change again, so the estimate is what grids use.
    fn estimated_main_size(&self) -> iced::Size {
        let (sidebar, right) = self.panel_widths();
        iced::Size::new(
            self.frame_inner() - sidebar - right,
            self.window_size.height,
        )
    }
    /// The panel in the right slot, if any.
    fn right_panel(&self) -> Option<splits::Right> {
        if self.queue {
            Some(splits::Right::Queue)
        } else if self.lyrics_open {
            Some(splits::Right::Lyrics)
        } else {
            None
        }
    }
    /// Width the sidebar, main view and right panel share: the window without the frame
    /// edges, the gaps between panels and the main panel's shadow margin.
    fn frame_inner(&self) -> f32 {
        let gaps: f32 = if self.right_panel().is_some() {
            2.0
        } else {
            1.0
        };
        self.window_size.width
            - 2.0 * self.frame_edge()
            - gaps * self.panel_gap()
            - 2.0 * styles::shadow_margin(&self.palette)
    }
    /// Shown sidebar and right panel widths (zero without a right panel).
    fn panel_widths(&self) -> (f32, f32) {
        self.splits.fit(self.frame_inner(), self.right_panel())
    }
    /// Width available to grids inside the main view.
    fn grid_width(&self) -> f32 {
        self.main_size.width - 2.0 * layout::CONTENT_PADDING
    }
    /// Card grid for the current width and theme type sizes.
    fn grid_layout(&self) -> layout::Grid {
        let sizes = self.palette.font_size;
        layout::grid(
            self.grid_width(),
            layout::Caption::new(sizes.body, sizes.small),
        )
    }
    /// Whether the open album or playlist is the context that is currently loaded in
    /// the player, so its header button pauses and resumes instead of restarting.
    fn page_is_playing_context(&self) -> bool {
        let Some(current) = &self.state.player.current else {
            return false;
        };
        // The Connect context is authoritative: it also covers playback another device
        // started, and a different context on a remote device rules out a stale local one.
        let connect: &ConnectState = &self.state.connect;
        if let Some(uri) = connect.context_uri.as_deref() {
            let matches: bool = match &self.query {
                Query::LikedTracks => devices::is_liked_context(uri),
                query => devices::context_uri(query).as_deref() == Some(uri),
            };
            if matches {
                return true;
            }
            if connect.is_remote() {
                return false;
            }
        }
        if self.play_context.as_ref() == Some(&self.query) {
            return true;
        }
        // After a restart only the track is known; an album page still matches its album.
        matches!(&self.query, Query::AlbumTracks { album_id }
            if current.album.as_ref().is_some_and(|a| &a.id == album_id))
    }
    /// Run a keyboard shortcut.
    fn shortcut(&mut self, shortcut: shortcuts::Shortcut) -> Task<Message> {
        use shortcuts::Shortcut;
        let player: &PlayerState = &self.state.player;
        let volume_ok: bool = devices::volume_supported(&self.state.connect);
        let action: Action = match shortcut {
            Shortcut::TogglePlayback => return self.reduce(Message::TogglePlayback),
            Shortcut::Previous => Action::Previous,
            Shortcut::Next => Action::Next,
            Shortcut::VolumeUp | Shortcut::VolumeDown if !volume_ok => return Task::none(),
            Shortcut::VolumeUp => Action::SetVolume(shortcuts::step_volume(player.volume, true)),
            Shortcut::VolumeDown => Action::SetVolume(shortcuts::step_volume(player.volume, false)),
            Shortcut::Shuffle => Action::SetShuffle(!player.shuffle),
            Shortcut::Repeat => Action::SetRepeat(next_repeat(player.repeat)),
            Shortcut::FocusSearch => return self.reduce(Message::FocusSearch),
            Shortcut::Back => return self.reduce(Message::Back),
            Shortcut::Forward => return self.reduce(Message::Forward),
            Shortcut::ZoomIn => return self.set_zoom(chrome::step_zoom(self.zoom, true)),
            Shortcut::ZoomOut => return self.set_zoom(chrome::step_zoom(self.zoom, false)),
            Shortcut::ZoomReset => return self.set_zoom(1.0),
            Shortcut::Escape => {
                if self.track_menu.is_some() {
                    self.track_menu = None;
                    return Task::none();
                }
                if self.devices {
                    return self.reduce(Message::Devices);
                }
                if self.settings {
                    return self.reduce(Message::Settings);
                }
                if self.queue {
                    return self.reduce(Message::Queue);
                }
                if self.lyrics_open {
                    return self.reduce(Message::Lyrics);
                }
                return Task::none();
            }
        };
        self.send(action);
        Task::none()
    }
    /// Whether an album grid belongs to an artist and gets the artist header.
    fn artist_page(&self) -> bool {
        matches!(self.query, Query::ArtistAlbums { .. })
    }
    /// Whether a track table gets the large header and play actions; search "Show all"
    /// songs get a plain section heading instead.
    fn track_hero(&self) -> bool {
        !matches!(
            self.query,
            Query::SearchTracks { .. } | Query::ArtistTopTracks { .. }
        )
    }
    /// Height of a shelf: a section heading over one row of cards.
    fn shelf_height(&self) -> f32 {
        layout::SECTION_HEADING + self.grid_layout().card_height + layout::GRID_GAP
    }
    /// Whether a block of the main page at `top` with `height` is (nearly) on screen.
    fn on_screen(&self, top: f32, height: f32) -> bool {
        layout::overlaps(top, height, self.scroll, self.main_size.height)
    }
    /// The search overview's top result for the current search text.
    fn top_result<'a>(&self, results: &'a SearchResults) -> Option<search::TopResult<'a>> {
        match &self.query {
            Query::Search { text } => search::top_result(results, text),
            _ => None,
        }
    }
    /// Top row of the search overview, aligned with the card grid below it.
    fn search_geometry(&self, songs: usize) -> layout::SearchTop {
        let p: &Palette = &self.palette;
        layout::SearchTop::new(
            self.grid_layout().width(),
            songs,
            p.shape.row_height,
            p.font_size.display,
            p.font_size.small,
        )
    }
    /// Search overview geometry: the top row and the tops of the artist and album shelves
    /// (when they have results).
    fn search_layout(&self, results: &SearchResults) -> SearchLayout {
        let shelf: f32 = self.shelf_height();
        let top: Option<layout::SearchTop> = self
            .top_result(results)
            .map(|_| self.search_geometry(results.tracks.len()));
        let mut y: f32 = top.map_or(0.0, |top| top.height());
        let mut place = |present: bool| -> Option<f32> {
            present.then(|| {
                y += shelf;
                y - shelf
            })
        };
        let artists: Option<f32> = place(!results.artists.is_empty());
        let albums: Option<f32> = place(!results.albums.is_empty());
        let playlists: Option<f32> = place(!results.playlists.is_empty());
        SearchLayout {
            top,
            artists,
            albums,
            playlists,
        }
    }
    /// Height of the artist page's Popular songs block: a heading over the song rows, one
    /// row of status while they load, and nothing when Spotify has none to show.
    fn popular_height(&self) -> f32 {
        let rows: usize = match &self.artist_top {
            Some(tracks) => tracks.len(),
            None if self.artist_top_failed => 0,
            None => 1,
        };
        match rows {
            0 => 0.0,
            rows => layout::SECTION_HEADING + rows as f32 * self.palette.shape.row_height,
        }
    }
    /// Fixed height of the blocks above the virtualized list of a page.
    fn content_top(&self, data: &PageData) -> f32 {
        match data {
            PageData::Playlists(_) | PageData::Artists(_) => layout::SECTION_HEADING,
            PageData::Albums(_) if self.artist_page() => {
                layout::HERO_HEIGHT + self.popular_height() + layout::SECTION_HEADING
            }
            PageData::Albums(_) => layout::SECTION_HEADING,
            PageData::Tracks(_) if self.track_hero() => {
                layout::HERO_HEIGHT
                    + layout::ACTIONS_HEIGHT
                    + layout::TABLE_HEADER
                    + layout::TRACK_TOOLS_HEIGHT
            }
            PageData::Tracks(_) => {
                layout::SECTION_HEADING + layout::TABLE_HEADER + layout::TRACK_TOOLS_HEIGHT
            }
            PageData::Search(_) | PageData::Queue { .. } => 0.0,
        }
    }
    /// Visible window of the main browser page in list slots, loaded or not: cards for
    /// playlists, artists and albums; fixed-height rows for tracks and queue entries. Home
    /// and the search overview show unvirtualized blocks: the window is their playlist
    /// shelf or song rows while that block is on screen.
    fn visible_slots(&self, data: &PageData) -> (usize, usize) {
        let len: usize = paging::page_info(data).0;
        let slots: usize = paging::slots(data);
        let block = |shown: usize, height: f32| {
            if self.on_screen(0.0, height) {
                (0, shown)
            } else {
                (0, 0)
            }
        };
        if self.home {
            // The Made For You shelf is above the playlist shelf.
            let shelf: f32 = self.shelf_height();
            return if self.on_screen(shelf, shelf) {
                (0, len.min(self.grid_layout().columns))
            } else {
                (0, 0)
            };
        }
        let offset: f32 = self.scroll - self.content_top(data);
        match data {
            PageData::Search(results) => match self.search_layout(results).top {
                Some(top) => block(results.tracks.len().min(layout::SEARCH_SONGS), top.height()),
                None => (0, 0),
            },
            PageData::Playlists(_) | PageData::Artists(_) | PageData::Albums(_) => {
                layout::visible_grid(slots, offset, self.main_size.height, self.grid_layout())
            }
            PageData::Tracks(_) | PageData::Queue { .. } => layout::visible_rows(
                slots,
                offset,
                self.main_size.height,
                self.palette.shape.row_height,
            ),
        }
    }
    /// The loaded items inside the visible window, as indices into the loaded run.
    /// Slots outside the run are placeholders and have no item or artwork.
    fn browser_window(&self, data: &PageData) -> (usize, usize) {
        let (start, end) = self.visible_slots(data);
        let first: usize = paging::first_offset(data) as usize;
        let loaded_end: usize = first + paging::page_info(data).0;
        let start: usize = start.clamp(first, loaded_end);
        (start - first, end.clamp(start, loaded_end) - first)
    }
    /// Header art of the current page: route art, or the album cover of its tracks.
    fn header_artwork(&self) -> Option<&str> {
        if let Some(url) = &self.route_artwork {
            return Some(url);
        }
        match (&self.query, &self.data) {
            (Query::AlbumTracks { .. }, Some(PageData::Tracks(p))) => {
                p.items.first().and_then(Track::artwork_url)
            }
            _ => None,
        }
    }
}
/// Vertical layout of the search overview, from the top of the page.
struct SearchLayout {
    /// Top result and songs row; absent without results.
    top: Option<layout::SearchTop>,
    artists: Option<f32>,
    albums: Option<f32>,
    playlists: Option<f32>,
}

/// Page query matching a playback context; single tracks have none.
/// Requests the app makes on its own as the view changes, rather than on a user's command.
fn is_background(action: &Action) -> bool {
    matches!(
        action,
        Action::Artwork { .. }
            | Action::Fetch { .. }
            | Action::CheckSaved(_)
            | Action::CancelRequest(_)
            | Action::SetVisible(_)
    )
}

/// Failures that resolve themselves, such as a busy backend, a rate limit or a dropped
/// connection, as opposed to ones that need the user, such as missing permission.
fn passing(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::Network
            | ErrorKind::RateLimited
            | ErrorKind::Unavailable
            | ErrorKind::StaleQueue
    )
}

fn context_query(source: &PlaySource) -> Option<Query> {
    match source {
        PlaySource::Playlist { playlist_id, .. } => Some(Query::PlaylistTracks {
            playlist_id: playlist_id.clone(),
        }),
        PlaySource::Album { album_id, .. } => Some(Query::AlbumTracks {
            album_id: album_id.clone(),
        }),
        PlaySource::LikedSongs { .. } => Some(Query::LikedTracks),
        PlaySource::Track(_) => None,
    }
}
/// Repeat mode after the repeat button or Ctrl+R: off, whole queue, one track.
fn next_repeat(mode: RepeatMode) -> RepeatMode {
    match mode {
        RepeatMode::Off => RepeatMode::Queue,
        RepeatMode::Queue => RepeatMode::One,
        RepeatMode::One => RepeatMode::Off,
    }
}
/// Join display-safe artist metadata.
fn artist_names(artists: &[Artist]) -> String {
    artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
/// Format a millisecond duration as minutes and seconds.
fn clock(ms: u64) -> String {
    format!("{}:{:02}", ms / 60000, ms / 1000 % 60)
}
/// Format a total duration the way album headers do: "1 hr 4 min" or "38 min 12 sec".
fn long_duration(ms: u64) -> String {
    let seconds: u64 = ms / 1000;
    let (hours, minutes) = (seconds / 3600, seconds / 60 % 60);
    if hours > 0 {
        format!("{hours} hr {minutes} min")
    } else {
        format!("{minutes} min {} sec", seconds % 60)
    }
}

/// Handle unconsumed keyboard and mouse shortcuts without intercepting text-field input.
/// Cursor moves and the release while a panel boundary is dragged.
fn split_drag(event: iced::Event, _: iced::event::Status, _: window::Id) -> Option<Message> {
    match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::SplitMove(position.x))
        }
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left))
        | iced::Event::Mouse(iced::mouse::Event::CursorLeft) => Some(Message::SplitEnd),
        _ => None,
    }
}

fn shortcut(event: iced::Event, status: iced::event::Status, _: window::Id) -> Option<Message> {
    if let iced::Event::Mouse(iced::mouse::Event::ButtonPressed(button)) = event {
        return match button {
            iced::mouse::Button::Back => Some(Message::Back),
            iced::mouse::Button::Forward => Some(Message::Forward),
            _ => None,
        };
    }
    if status != iced::event::Status::Ignored {
        return None;
    }
    if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
        if modifiers.is_empty() {
            match key {
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown) => {
                    return Some(Message::RowMove(true));
                }
                iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp) => {
                    return Some(Message::RowMove(false));
                }
                iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) => {
                    return Some(Message::PlaySelected);
                }
                _ => {}
            }
        }
        return shortcuts::map(&key, modifiers).map(Message::Shortcut);
    }
    None
}

#[cfg(test)]
mod channel_tests {
    use super::*;
    /// Build only real contract state; no fabricated library enters the production path.
    pub(super) fn fixture() -> (Ui, BackendPort) {
        let initial: AppState = AppState {
            settings: crate::domain::Settings {
                client_id: Some("public-client".into()),
                cache_limit_bytes: 123_000_000,
                theme_path: None,
                autoplay: true,
            },
            auth: AuthState::SignedOut,
            player: PlayerState {
                status: PlaybackStatus::Paused,
                current: None,
                current_entry_id: None,
                position_ms: 0,
                observed_at: std::time::Instant::now(),
                volume: 0.5,
                shuffle: false,
                repeat: RepeatMode::Off,
                queue_revision: 7,
                seek_revision: 0,
            },
            connect: crate::domain::ConnectState::default(),
            cache_used_bytes: 0,
            cache_limit_bytes: 123_000_000,
            lyrics_key_set: false,
            discover_active: false,
        };
        let (front, back): (FrontendPort, BackendPort) = channels(initial);
        let (ui, _task): (Ui, Task<Message>) = Ui::boot(front);
        (ui, back)
    }
    #[test]
    fn saved_route_waits_for_verified_account() {
        let (mut ui, _back) = fixture();
        ui.pending_preferences = Some(serde_json::from_str(
            r#"{"version":1,"owner":"account","kind":"playlist","key":"saved","title":"Saved playlist","offset":100,"width":1200,"height":800}"#,
        ).unwrap());
        let mut state = ui.state.clone();
        state.auth = AuthState::Ready {
            user_id: "account".into(),
        };
        let _ = ui.update(Message::State(state));
        assert!(
            matches!(&ui.query, Query::PlaylistTracks { playlist_id } if playlist_id.0 == "saved")
        );
        assert_eq!(ui.offset, 100);
        assert!(ui.pending_preferences.is_none());
    }

    #[test]
    fn connect_warning_survives_later_notices_until_dismissed() {
        let (mut ui, _back) = fixture();
        ui.notice.clear();
        let warning = "Spotify Connect unavailable. This computer can still play locally.";
        let error = |message: &str| AppError {
            kind: ErrorKind::Unavailable,
            message: message.into(),
            retry_after_seconds: None,
        };
        let _ = ui.update(Message::Backend(Event::Notice(error(warning))));
        let _ = ui.update(Message::Backend(Event::Notice(error("Polling failed"))));
        assert_eq!(ui.notice, warning);
        let _ = ui.update(Message::DismissNotice);
        assert_eq!(ui.notice, "Polling failed");
        let _ = ui.update(Message::DismissNotice);
        assert!(ui.notice.is_empty());
    }

    /// Startup configuration replaces boot defaults while player ticks preserve drafts.
    #[test]
    fn startup_settings_and_drafts() {
        let (mut ui, _back) = fixture();
        let mut state = ui.state.clone();
        state.settings.client_id = Some("persisted-client".into());
        state.settings.cache_limit_bytes = 456_000_000;
        let _ = ui.update(Message::State(state.clone()));
        assert_eq!(ui.client, "persisted-client");
        assert_eq!(ui.cache, "456000000");
        let _ = ui.update(Message::Client("unfinished edit".into()));
        state.player.position_ms += 100;
        let _ = ui.update(Message::State(state));
        assert_eq!(ui.client, "unfinished edit");
    }
    /// Disconnect exposes an explicit uncheckpointed exit rather than waiting for QuitReady.
    #[test]
    fn disconnected_quit_and_close_exit() {
        let (mut ui, mut back) = fixture();
        let _ = ui.update(Message::Action(Action::Quit));
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::Quit
        ));
        let _ = ui.update(Message::Disconnected);
        assert!(ui.notice.contains("checkpoint not confirmed"));
        assert!(ui.disconnected);
        let task = ui.update(Message::Action(Action::Quit));
        assert!(task.units() > 0);
        assert!(back.commands.try_recv().is_err());
        let task = ui.update(Message::Window(
            window::Id::unique(),
            window::Event::CloseRequested,
        ));
        assert!(task.units() > 0);
    }
    /// Identity transitions discard old account pages and invalidate pending responses.
    #[test]
    fn identity_change_clears_account_data() {
        let (mut ui, _back) = fixture();
        ui.state.auth = AuthState::Ready {
            user_id: "old".into(),
        };
        ui.sidebar_data = Some(PageData::Playlists(Page {
            offset: 0,
            total: Some(0),
            items: vec![],
            has_more: false,
            next_offset: 0,
        }));
        ui.query = Query::PlaylistTracks {
            playlist_id: SpotifyId("oldplaylist".into()),
        };
        ui.fetch();
        let id = ui.pending.unwrap();
        let mut state = ui.state.clone();
        state.auth = AuthState::SignedOut;
        let _ = ui.update(Message::State(state));
        assert!(ui.sidebar_data.is_none());
        assert!(ui.data.is_none());
        assert!(!ui.active.contains(&id));
        assert!(matches!(ui.query, Query::OwnPlaylists));
    }
    /// A cancelled request cannot replace a newer status notice.
    #[test]
    fn stale_errors_are_ignored() {
        let (mut ui, _back) = fixture();
        ui.fetch();
        let id = ui.pending.unwrap();
        ui.fetch();
        ui.notice = "current request".into();
        let _ = ui.update(Message::Backend(Event::Completed {
            id,
            result: Err(AppError {
                kind: ErrorKind::Network,
                message: "obsolete".into(),
                retry_after_seconds: None,
            }),
        }));
        assert_eq!(ui.notice, "current request");
    }
    #[test]
    fn commands_are_real_and_monotonic() {
        let (mut ui, mut back) = fixture();
        ui.send(Action::Pause);
        ui.send(Action::Next);
        let first: Command = back.commands.try_recv().unwrap();
        let second: Command = back.commands.try_recv().unwrap();
        assert!(matches!(first.action, Action::Pause));
        assert!(second.id.0 > first.id.0);
    }
    #[test]
    fn backpressure_is_visible() {
        let (mut ui, _back) = fixture();
        for _ in 0..COMMAND_CAPACITY {
            assert!(ui.send(Action::Pause).is_some());
        }
        assert!(ui.send(Action::Pause).is_none());
        assert!(ui.notice.contains("busy"));
    }
    /// Background loads that will be retried fail silently; user commands still report.
    #[test]
    fn passing_background_failures_are_not_shown() {
        let (mut ui, _back) = fixture();
        ui.notice.clear();
        let busy = || AppError {
            kind: ErrorKind::Unavailable,
            message: "backend is busy".into(),
            retry_after_seconds: None,
        };
        let page = PageRequest {
            offset: 0,
            limit: 20,
        };
        let fetch = ui
            .send(Action::Fetch {
                query: Query::LikedTracks,
                page,
            })
            .unwrap();
        let _ = ui.update(Message::Backend(Event::Completed {
            id: fetch,
            result: Err(busy()),
        }));
        assert!(ui.notice.is_empty());
        let pause = ui.send(Action::Pause).unwrap();
        let _ = ui.update(Message::Backend(Event::Completed {
            id: pause,
            result: Err(busy()),
        }));
        assert_eq!(ui.notice, "backend is busy");
    }
    #[test]
    fn background_requests_are_dropped_quietly_under_backpressure() {
        let (mut ui, _back) = fixture();
        ui.notice.clear();
        for _ in 0..COMMAND_CAPACITY {
            assert!(ui.send(Action::Pause).is_some());
        }
        assert!(ui.send(Action::SetVisible(true)).is_none());
        assert!(ui.notice.is_empty());
    }
    #[test]
    fn notices_dismiss_themselves() {
        let (mut ui, _back) = fixture();
        ui.notice.clear();
        let _ = ui.update(Message::Backend(Event::Notice(AppError {
            kind: ErrorKind::Network,
            message: "Spotify is unreachable".into(),
            retry_after_seconds: None,
        })));
        let now = std::time::Instant::now();
        let _ = ui.update(Message::NoticeTick(now));
        assert_eq!(ui.notice, "Spotify is unreachable");
        let _ = ui.update(Message::NoticeTick(now + NOTICE_DURATION));
        assert!(ui.notice.is_empty());
    }
    #[test]
    fn stale_pages_cannot_replace_new_query() {
        let (mut ui, _back) = fixture();
        ui.fetch();
        let old: RequestId = ui.pending.unwrap();
        ui.fetch();
        let _ = ui.update(Message::Backend(Event::Completed {
            id: old,
            result: Ok(Response::Page(PageData::Artists(Page {
                offset: 0,
                total: Some(0),
                items: vec![],
                has_more: false,
                next_offset: 0,
            }))),
        }));
        assert!(ui.data.is_none());
        assert_ne!(ui.pending, Some(old));
    }
    #[test]
    fn closing_the_window_quits() {
        let (mut ui, mut back) = fixture();
        let id: window::Id = window::Id::unique();
        ui.window = Some(id);
        let _ = ui.update(Message::Window(id, window::Event::CloseRequested));
        let command: Command = back.commands.try_recv().unwrap();
        assert!(matches!(command.action, Action::Quit));
    }
    #[test]
    fn settings_preserve_authoritative_client_and_budget() {
        let (mut ui, mut back) = fixture();
        let _ = ui.update(Message::Save);
        let command: Command = back.commands.try_recv().unwrap();
        if let Action::Configure(settings) = command.action {
            assert_eq!(settings.cache_limit_bytes, 123_000_000);
            assert_eq!(settings.client_id.as_deref(), Some("public-client"));
        } else {
            panic!("expected configure")
        }
    }
    #[test]
    fn queue_revision_is_checked() {
        let (mut ui, _back) = fixture();
        ui.fetch_queue();
        let id: RequestId = ui.queue_pending.unwrap();
        let _ = ui.update(Message::Backend(Event::Completed {
            id,
            result: Ok(Response::Page(PageData::Queue {
                revision: 6,
                page: Page {
                    offset: 0,
                    total: Some(0),
                    items: vec![],
                    has_more: false,
                    next_offset: 0,
                },
            })),
        }));
        assert!(ui.queue_data.is_none());
    }
    #[test]
    fn sidebar_pages_do_not_replace_main_browser() {
        let (mut ui, mut back) = fixture();
        ui.query = Query::SearchArtists {
            text: "Miles".into(),
        };
        ui.fetch_sidebar();
        let command = back.commands.try_recv().unwrap();
        assert!(matches!(
            command.action,
            Action::Fetch {
                query: Query::OwnPlaylists,
                ..
            }
        ));
        let _ = ui.update(Message::Backend(Event::Completed {
            id: command.id,
            result: Ok(Response::Page(PageData::Playlists(Page {
                offset: 0,
                items: vec![],
                total: Some(0),
                has_more: false,
                next_offset: 0,
            }))),
        }));
        assert!(ui.sidebar_data.is_some());
        assert!(ui.data.is_none());
        let _ = ui.update(Message::SidebarFilter("jazz".into()));
        assert!(matches!(ui.query, Query::SearchArtists { .. }));
        assert!(back.commands.try_recv().is_err());
    }
    #[test]
    fn hidden_window_never_requests_artwork() {
        let (mut ui, mut back) = fixture();
        ui.window = None;
        ui.fetch_artwork();
        assert!(back.commands.try_recv().is_err());
    }
    /// Only the visible grid window is requested, bounded in flight, and scrolling cancels.
    #[test]
    fn visible_cards_request_bounded_artwork_and_cancel_when_scrolled_away() {
        let (mut ui, mut back) = fixture();
        ui.window = Some(window::Id::unique());
        // The full playlist grid; Home would show only its first row.
        ui.home = false;
        ui.data = Some(PageData::Playlists(Page {
            offset: 0,
            total: Some(100),
            items: (0..100)
                .map(|i| Playlist {
                    id: SpotifyId(format!("playlist{i}")),
                    name: format!("Playlist {i}"),
                    owner_id: "me".into(),
                    snapshot_id: String::new(),
                    track_count: None,
                    artwork_url: Some(format!("art{i}")),
                })
                .collect(),
            has_more: false,
            next_offset: 100,
        }));
        ui.fetch_artwork();
        let mut first = Vec::new();
        while let Ok(command) = back.commands.try_recv() {
            match command.action {
                Action::Artwork { url, .. } => first.push((command.id, url)),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(first.len(), ARTWORK_IN_FLIGHT);
        assert_eq!(first[0].1, "art0");
        ui.scroll = 3_000.0;
        ui.fetch_artwork();
        let mut cancelled = 0;
        let mut requested = Vec::new();
        while let Ok(command) = back.commands.try_recv() {
            match command.action {
                Action::CancelRequest(_) => cancelled += 1,
                Action::Artwork { url, .. } => requested.push(url),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(cancelled, ARTWORK_IN_FLIGHT);
        assert_eq!(requested.len(), ARTWORK_IN_FLIGHT);
        assert!(!requested.contains(&"art0".to_string()));
        // A late completion for a cancelled request is ignored rather than cached.
        assert!(ui.complete_artwork(first[0].0, None).is_none());
    }
}

#[cfg(test)]
mod browsing_tests {
    use super::*;
    #[test]
    fn artist_load_failure_stays_visible_and_can_be_retried() {
        let (mut ui, mut back) = channel_tests::fixture();
        ui.search = "keys".into();
        let _ = ui.update(Message::SearchNow);
        let search = ui.route();
        while back.commands.try_recv().is_ok() {}
        let artist = Route::artist(&Artist {
            id: SpotifyId("artist".into()),
            name: "Keys".into(),
            artwork_url: None,
        });
        let _ = ui.update(Message::Navigate(artist.clone()));
        let failed = ui.pending.unwrap();
        let _ = ui.update(Message::Backend(Event::Completed {
            id: failed,
            result: Err(AppError {
                kind: ErrorKind::Network,
                message: "Spotify API request failed".into(),
                retry_after_seconds: None,
            }),
        }));
        let _ = ui.update(Message::DismissNotice);
        assert_eq!(ui.page_error.as_deref(), Some("Spotify API request failed"));
        assert!(ui.data.is_none() && ui.pending.is_none());

        while back.commands.try_recv().is_ok() {}
        let _ = ui.update(Message::RetryPage);
        let retry = back.commands.try_recv().unwrap();
        assert!(matches!(retry.action, Action::Fetch { ref query, .. } if *query == artist.query));
        assert_eq!(ui.pending, Some(retry.id));
        assert!(ui.page_error.is_none());
        complete(
            &mut ui,
            retry.id,
            Response::Page(PageData::Albums(Page {
                offset: 0,
                items: vec![],
                total: Some(0),
                has_more: false,
                next_offset: 0,
            })),
        );
        assert!(matches!(ui.data, Some(PageData::Albums(_))));
        assert_eq!(ui.route(), artist);
        let _ = ui.update(Message::Back);
        assert_eq!(ui.route(), search);
        assert!(ui.page_error.is_none());
    }

    fn artists(offset: u64, len: usize, has_more: bool) -> Response {
        artists_of(offset, len, None, has_more)
    }
    fn artists_of(offset: u64, len: usize, total: Option<u64>, has_more: bool) -> Response {
        Response::Page(PageData::Artists(Page {
            offset,
            total,
            items: (0..len)
                .map(|i| Artist {
                    id: SpotifyId(format!("artist{}", offset as usize + i)),
                    name: String::new(),
                    artwork_url: None,
                })
                .collect(),
            has_more,
            next_offset: offset + len as u64,
        }))
    }
    fn complete(ui: &mut Ui, id: RequestId, response: Response) {
        let _ = ui.update(Message::Backend(Event::Completed {
            id,
            result: Ok(response),
        }));
    }
    /// Scrolling to the end appends the next bounded page; a newer query discards it.
    #[test]
    fn load_more_appends_and_respects_generation() {
        let (mut ui, mut back) = channel_tests::fixture();
        ui.home = false;
        ui.fetch();
        let first: Command = back.commands.try_recv().unwrap();
        complete(&mut ui, first.id, artists(0, 100, true));
        let _ = ui.update(Message::Scroll(50_000.0, 700.0));
        let more: Command = back.commands.try_recv().unwrap();
        assert!(matches!(
            more.action,
            Action::Fetch {
                page: PageRequest { offset: 100, .. },
                ..
            }
        ));
        // A second trigger while loading does not duplicate the request.
        let _ = ui.update(Message::Scroll(50_100.0, 700.0));
        assert!(back.commands.try_recv().is_err());
        complete(&mut ui, more.id, artists(100, 50, false));
        assert_eq!(ui.data.as_ref().map(|d| paging::page_info(d).0), Some(150));
        ui.fetch();
        let replaced: Command = back.commands.try_recv().unwrap();
        complete(&mut ui, replaced.id, artists(0, 100, true));
        let _ = ui.update(Message::Scroll(50_000.0, 700.0));
        let stale: Command = back.commands.try_recv().unwrap();
        ui.fetch();
        complete(&mut ui, stale.id, artists(100, 100, false));
        assert!(ui.data.is_none());
    }
    /// Regression: owner-filtered playlist pages and track pages whose rows were all
    /// dropped continue after the raw upstream page, so pages neither repeat nor stall.
    #[test]
    fn filtered_pages_continue_after_raw_rows() {
        fn fetched_offset(back: &mut BackendPort) -> (RequestId, u64) {
            let command: Command = back.commands.try_recv().unwrap();
            match command.action {
                Action::Fetch { page, .. } => (command.id, page.offset),
                other => panic!("unexpected {other:?}"),
            }
        }
        let (mut ui, mut back) = channel_tests::fixture();
        ui.fetch_sidebar();
        let (id, _) = fetched_offset(&mut back);
        // 100 raw playlists, of which the user owns 30.
        let owned = |offset: u64| Playlist {
            id: SpotifyId(format!("p{offset}")),
            name: String::new(),
            owner_id: "me".into(),
            snapshot_id: String::new(),
            track_count: None,
            artwork_url: None,
        };
        complete(
            &mut ui,
            id,
            Response::Page(PageData::Playlists(Page {
                offset: 0,
                total: Some(300),
                items: (0..30).map(owned).collect(),
                has_more: true,
                next_offset: 100,
            })),
        );
        let _ = ui.update(Message::SidebarMore);
        let (id, offset) = fetched_offset(&mut back);
        assert_eq!(offset, 100);
        complete(
            &mut ui,
            id,
            Response::Page(PageData::Playlists(Page {
                offset: 100,
                total: Some(300),
                items: (100..130).map(owned).collect(),
                has_more: true,
                next_offset: 200,
            })),
        );
        assert_eq!(ui.sidebar_items().len(), 60);

        ui.query = Query::PlaylistTracks {
            playlist_id: SpotifyId("local-files".into()),
        };
        ui.home = false;
        ui.fetch();
        let (id, _) = fetched_offset(&mut back);
        // The first 100 raw rows were all local files.
        complete(
            &mut ui,
            id,
            Response::Page(PageData::Tracks(Page {
                offset: 0,
                total: Some(101),
                items: vec![],
                has_more: true,
                next_offset: 100,
            })),
        );
        // The empty page leaves one placeholder row in view, which asks for the rest.
        assert_eq!(fetched_offset(&mut back).1, 100);
    }
    /// The list is as long as the total from the first page on; jumping away from the loaded
    /// run replaces it with the page under the viewport, and scrolling back up replaces it
    /// again, so any position is reachable without loading what lies between.
    #[test]
    fn scrolling_jumps_to_any_position_of_a_known_total() {
        let (mut ui, mut back) = channel_tests::fixture();
        ui.home = false;
        ui.fetch();
        let first: Command = back.commands.try_recv().unwrap();
        complete(&mut ui, first.id, artists_of(0, 100, Some(5_000), true));
        let slots = |ui: &Ui| ui.data.as_ref().map(paging::slots);
        assert_eq!(slots(&ui), Some(5_000));
        let fetch = |back: &mut BackendPort| match back.commands.try_recv().unwrap() {
            Command {
                id,
                action: Action::Fetch { page, .. },
            } => (id, page.offset),
            other => panic!("unexpected {other:?}"),
        };
        let _ = ui.update(Message::Scroll(1_000_000.0, 700.0));
        let (id, offset) = fetch(&mut back);
        assert!(offset > 100, "jumped past the loaded run, got {offset}");
        let len = 100.min(5_000 - offset as usize);
        complete(
            &mut ui,
            id,
            artists_of(offset, len, Some(5_000), offset + 100 < 5_000),
        );
        let data = ui.data.as_ref().unwrap();
        assert_eq!(paging::first_offset(data), offset);
        assert_eq!(slots(&ui), Some(5_000));
        // Back at the top, the first page is requested again.
        let _ = ui.update(Message::Scroll(0.0, 700.0));
        let (_, offset) = fetch(&mut back);
        assert_eq!(offset, 0);
    }
    /// Back and Forward restore routes; refining a search does not add history entries.
    #[test]
    fn history_follows_navigation() {
        let (mut ui, _back) = channel_tests::fixture();
        let album: Route = Route::browse(
            Query::AlbumTracks {
                album_id: SpotifyId("album".into()),
            },
            "Album",
            Some("art".into()),
        );
        let _ = ui.update(Message::Navigate(album.clone()));
        ui.search = "first".into();
        let _ = ui.update(Message::SearchNow);
        ui.search = "second".into();
        let _ = ui.update(Message::SearchNow);
        let _ = ui.update(Message::Back);
        assert_eq!(ui.route(), album);
        let _ = ui.update(Message::Back);
        assert!(matches!(ui.query, Query::OwnPlaylists));
        let _ = ui.update(Message::Forward);
        assert_eq!(ui.route_artwork.as_deref(), Some("art"));
        let _ = ui.update(Message::Settings);
        assert!(ui.settings && matches!(ui.query, Query::AlbumTracks { .. }));
        let _ = ui.update(Message::Back);
        assert!(!ui.settings);
    }
    /// The header button of the page that started playback toggles it; Escape closes
    /// Settings before the queue.
    #[test]
    fn header_play_follows_context_and_escape_closes_panels() {
        let (mut ui, _back) = channel_tests::fixture();
        let playlist = SpotifyId("mix".into());
        let _ = ui.update(Message::Navigate(Route::browse(
            Query::PlaylistTracks {
                playlist_id: playlist.clone(),
            },
            "Mix",
            None,
        )));
        ui.state.player.current = Some(Track {
            id: SpotifyId("track".into()),
            name: "Track".into(),
            artists: Vec::new(),
            album: None,
            duration_ms: 1000,
            playable: true,
            source_index: None,
        });
        assert!(!ui.page_is_playing_context());
        let _ = ui.update(Message::Action(Action::Play(PlaySource::Playlist {
            playlist_id: playlist,
            start_offset: 3,
        })));
        assert!(ui.page_is_playing_context());
        let _ = ui.update(Message::Action(Action::Play(PlaySource::Track(SpotifyId(
            "other".into(),
        )))));
        assert!(!ui.page_is_playing_context());

        let _ = ui.update(Message::Queue);
        let _ = ui.update(Message::Settings);
        let _ = ui.update(Message::Shortcut(shortcuts::Shortcut::Escape));
        assert!(!ui.settings && ui.queue);
        let _ = ui.update(Message::Shortcut(shortcuts::Shortcut::Escape));
        assert!(!ui.queue);
    }

    /// The picker refreshes devices when it opens, Escape closes it, and a pick transfers
    /// playback and closes it. A remote device only hears a slider's final value.
    #[test]
    fn device_picker_and_remote_sliders() {
        let (mut ui, mut back) = channel_tests::fixture();
        let _ = ui.update(Message::Devices);
        assert!(ui.devices);
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::RefreshDevices
        ));
        let _ = ui.update(Message::Shortcut(shortcuts::Shortcut::Escape));
        assert!(!ui.devices && back.commands.try_recv().is_err());

        let _ = ui.update(Message::Devices);
        let _ = back.commands.try_recv();
        let _ = ui.update(Message::Transfer("kitchen".into()));
        assert!(!ui.devices);
        match back.commands.try_recv().unwrap().action {
            Action::TransferPlayback { device_id, play } => {
                assert_eq!((device_id.as_str(), play), ("kitchen", true));
            }
            other => panic!("unexpected {other:?}"),
        }

        // Local playback sends every slider step.
        let _ = ui.update(Message::Volume(0.2));
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::SetVolume(v) if v == 0.2
        ));
        ui.state.connect.this_device_id = Some("here".into());
        ui.state.connect.active_device_id = Some("kitchen".into());
        let _ = ui.update(Message::Volume(0.3));
        let _ = ui.update(Message::Volume(0.4));
        let _ = ui.update(Message::Seek(5000));
        assert!(back.commands.try_recv().is_err());
        let _ = ui.update(Message::VolumeReleased);
        let _ = ui.update(Message::SeekReleased);
        let _ = ui.update(Message::VolumeReleased);
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::SetVolume(v) if v == 0.4
        ));
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::Seek { position_ms: 5000 }
        ));
        assert!(back.commands.try_recv().is_err());
    }

    /// Scrubbing seeks once, on release, and the slider stays where it was dropped until
    /// the player reports that position, so it never snaps back to the old one.
    #[test]
    fn scrubbing_seeks_on_release_and_holds_until_the_player_arrives() {
        let (mut ui, mut back) = channel_tests::fixture();
        let mut state: AppState = ui.state.clone();
        state.player.current = Some(Track {
            duration_ms: 200_000,
            ..track("a", None)
        });
        state.player.position_ms = 1000;
        let _ = ui.update(Message::State(state.clone()));
        while back.commands.try_recv().is_ok() {}

        for position_ms in [20_000, 40_000, 60_000] {
            let _ = ui.update(Message::Seek(position_ms));
        }
        assert!(back.commands.try_recv().is_err());
        let _ = ui.update(Message::SeekReleased);
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::Seek {
                position_ms: 60_000
            }
        ));

        // An update sent before the backend handled the seek keeps the dropped position.
        let _ = ui.update(Message::State(state.clone()));
        assert_eq!(ui.seek_hold.as_ref().and_then(|h| h.shown()), Some(60_000));
        state.player.position_ms = 60_000;
        let _ = ui.update(Message::State(state));
        assert!(ui.seek_hold.is_none());
    }

    fn track(id: &str, source_index: Option<u64>) -> Track {
        Track {
            id: SpotifyId(id.into()),
            name: id.into(),
            artists: Vec::new(),
            album: None,
            duration_ms: 1000,
            playable: true,
            source_index,
        }
    }

    /// A clicked track shows as playing at once, stays through stale backend updates and
    /// gives way to the backend's view if the request fails.
    #[test]
    fn clicked_track_shows_before_the_backend_confirms_it() {
        let (mut ui, mut back) = channel_tests::fixture();
        let source = PlaySource::Track(SpotifyId("a".into()));
        let _ = ui.update(Message::PlayTrack(source, Box::new(track("a", None))));
        let id = back.commands.try_recv().unwrap().id;
        assert_eq!(ui.state.player.current.as_ref().unwrap().id.0, "a");
        assert_eq!(ui.state.player.status, PlaybackStatus::Playing);

        let mut stale: AppState = ui.state.clone();
        stale.player.current = Some(track("old", None));
        stale.player.status = PlaybackStatus::Paused;
        let _ = ui.update(Message::State(stale));
        assert_eq!(ui.state.player.current.as_ref().unwrap().id.0, "a");

        let _ = ui.update(Message::Backend(Event::Completed {
            id,
            result: Err(AppError {
                kind: ErrorKind::Unavailable,
                message: "unavailable".into(),
                retry_after_seconds: None,
            }),
        }));
        assert_eq!(ui.state.player.current.as_ref().unwrap().id.0, "old");
        assert_eq!(ui.state.player.status, PlaybackStatus::Paused);
    }

    /// Filtering/sorting selects source occurrences, including repeated track IDs, and
    /// loading another page follows the raw continuation instead of a display index.
    #[test]
    fn filtered_keyboard_play_and_load_more_keep_source_occurrences() {
        let (mut ui, mut back) = channel_tests::fixture();
        ui.home = false;
        ui.query = Query::LikedTracks;
        let mut items = vec![
            track("same", Some(100)),
            track("same", Some(102)),
            track("same", Some(105)),
        ];
        items[0].name = "Zebra".into();
        items[1].name = "Alpha".into();
        items[2].name = "Alpha".into();
        ui.data = Some(PageData::Tracks(Page {
            offset: 100,
            total: Some(200),
            items,
            has_more: true,
            next_offset: 106,
        }));
        let _ = ui.update(Message::TrackFilter("ALP".into()));
        let _ = ui.update(Message::TrackSort(track_filter::Sort::Title));
        let _ = ui.update(Message::RowMove(true));
        assert_eq!(ui.selected_row, Some(101));
        let _ = ui.update(Message::RowMove(true));
        assert_eq!(ui.selected_row, Some(102));
        let _ = ui.update(Message::PlaySelected);
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::Play(PlaySource::LikedSongs { start_offset: 105 })
        ));
        let _ = ui.update(Message::TrackMore);
        let command = back.commands.try_recv().unwrap();
        assert!(matches!(
            command.action,
            Action::Fetch {
                page: PageRequest { offset: 106, .. },
                ..
            }
        ));
        complete(
            &mut ui,
            command.id,
            Response::Page(PageData::Tracks(Page {
                offset: 106,
                total: Some(200),
                items: vec![track("Alpha", Some(110))],
                has_more: false,
                next_offset: 200,
            })),
        );
        assert_eq!(ui.track_filter, "ALP");
        assert_eq!(paging::page_info(ui.data.as_ref().unwrap()).0, 4);
        assert!(
            back.commands.try_recv().is_err(),
            "filter does not request unfiltered viewport pages"
        );
        ui.settings = true;
        let _ = ui.update(Message::PlaySelected);
        assert!(
            back.commands.try_recv().is_err(),
            "Settings must not play a hidden selection"
        );
    }

    /// Regression: the backend skips local files, so a row plays from its raw position.
    #[test]
    fn rows_play_from_their_source_position() {
        let (mut ui, _back) = channel_tests::fixture();
        // Raw rows: 100 "a", 101 local file (skipped), 102 "b".
        let items = vec![track("a", Some(100)), track("b", Some(102))];
        ui.query = Query::PlaylistTracks {
            playlist_id: SpotifyId("mix".into()),
        };
        assert!(matches!(
            ui.source(&items, 100, 1),
            PlaySource::Playlist {
                start_offset: 102,
                ..
            }
        ));
        ui.query = Query::LikedTracks;
        assert!(matches!(
            ui.source(&items, 100, 1),
            PlaySource::LikedSongs { start_offset: 102 }
        ));
        ui.query = Query::Search { text: "b".into() };
        assert!(matches!(ui.source(&items, 0, 1), PlaySource::Track(id) if id.0 == "b"));
    }

    /// Typing searches the mixed overview and refines it in place; clearing the field
    /// shows the start page without asking the backend for an empty search.
    #[test]
    fn search_overview_routes() {
        let (mut ui, mut back) = channel_tests::fixture();
        ui.search = "tram".into();
        let _ = ui.update(Message::SearchNow);
        let command = back.commands.try_recv().unwrap();
        assert!(matches!(
            command.action,
            Action::Fetch { query: Query::Search { ref text }, .. } if text == "tram"
        ));
        let _ = ui.update(Message::Navigate(Route::browse(
            Query::SearchAlbums {
                text: "tram".into(),
            },
            "Albums for “tram”",
            None,
        )));
        ui.search = "  ".into();
        let _ = ui.update(Message::SearchNow);
        assert!(matches!(&ui.query, Query::Search { text } if text.is_empty()));
        while let Ok(command) = back.commands.try_recv() {
            assert!(!matches!(
                command.action,
                Action::Fetch {
                    query: Query::Search { .. },
                    ..
                }
            ));
        }
        assert!(ui.data.is_none() && ui.pending.is_none());
        // Clearing replaced the "Show all" page; Back returns to the overview and its text.
        let _ = ui.update(Message::Back);
        assert!(matches!(&ui.query, Query::Search { text } if text == "tram"));
        assert_eq!(ui.search, "tram");
    }

    /// Saved routes restore the new pages; older "search" and "library" kinds still load.
    #[test]
    fn saved_search_and_liked_routes_restore() {
        let (mut ui, _back) = channel_tests::fixture();
        ui.state.auth = AuthState::Ready {
            user_id: "account".into(),
        };
        let restore = |ui: &mut Ui, kind: &str, key: &str| {
            let json = format!(
                r#"{{"version":1,"owner":"account","kind":"{kind}","key":"{key}","title":"t","offset":0,"width":1200,"height":800}}"#
            );
            serde_json::from_str::<persistence::Preferences>(&json)
                .unwrap()
                .apply(ui);
        };
        restore(&mut ui, "search", "tram");
        assert!(matches!(&ui.query, Query::Search { text } if text == "tram"));
        assert_eq!((ui.title.as_str(), ui.search.as_str()), ("tram", "tram"));
        restore(&mut ui, "liked", "");
        assert!(matches!(ui.query, Query::LikedTracks) && !ui.home);
        restore(&mut ui, "library", "");
        assert!(matches!(ui.query, Query::OwnPlaylists) && ui.home);
        restore(&mut ui, "playlists", "");
        assert!(matches!(ui.query, Query::OwnPlaylists) && !ui.home);
    }

    /// Home asks for the saved-albums shelf once; a failure is shown, not retried in a loop.
    #[test]
    fn home_shelf_loads_once_and_failures_do_not_retry() {
        let (mut ui, mut back) = channel_tests::fixture();
        ui.window = Some(window::Id::unique());
        ui.state.auth = AuthState::Ready {
            user_id: "account".into(),
        };
        let _ = ui.update(Message::Tick);
        let shelf = back.commands.try_recv().unwrap();
        assert!(matches!(
            shelf.action,
            Action::Fetch {
                query: Query::SavedAlbums,
                ..
            }
        ));
        let _ = ui.update(Message::Backend(Event::Completed {
            id: shelf.id,
            result: Err(AppError {
                kind: ErrorKind::Permission,
                message: "sign in again".into(),
                retry_after_seconds: None,
            }),
        }));
        assert!(ui.shelf_failed && ui.notice == "sign in again");
        let _ = ui.update(Message::Tick);
        assert!(back.commands.try_recv().is_err());
        let _ = ui.update(Message::RetryShelves);
        assert!(!ui.shelf_failed);
        assert!(matches!(
            back.commands.try_recv().unwrap().action,
            Action::Fetch {
                query: Query::SavedAlbums,
                ..
            }
        ));
        let _ = ui.update(Message::Tick);
        assert!(
            back.commands.try_recv().is_err(),
            "retry stays single-flight"
        );
    }
}
