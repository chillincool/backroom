//! Tabs (libadwaita's tab view; the tab bar shows from two tabs) and each
//! tab's split panes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use backroom_app::app::{AppState, Changes};
use backroom_app::buffer::BufferId;
use backroom_app::panes::{PaneNode, SplitAxis};
use gtk::glib;
use uuid::Uuid;

use crate::desktop::controller::Controller;
use crate::desktop::messages::BufferPane;
use crate::desktop::style::Palette;

struct Page {
    page: adw::TabPage,
    host: adw::Bin,
    /// The pane tree's shape when it was built; rebuilt when it changes.
    shape: String,
}

pub struct Tabs {
    pub view: adw::TabView,
    pub bar: adw::TabBar,
    pages: RefCell<HashMap<Uuid, Page>>,
    panes: RefCell<HashMap<Uuid, BufferPane>>,
    /// Set while the view is changed to match the state, so its signals
    /// aren't taken for the user's.
    syncing: Rc<Cell<bool>>,
}

impl Tabs {
    pub fn new(ctrl: &Rc<Controller>) -> Rc<Self> {
        let view = adw::TabView::new();
        let bar = adw::TabBar::builder().view(&view).autohide(true).build();
        let syncing = Rc::new(Cell::new(false));
        let tabs = Rc::new(Tabs {
            view,
            bar,
            pages: RefCell::default(),
            panes: RefCell::default(),
            syncing,
        });
        tabs.connect(ctrl);
        tabs
    }

    fn id_of(&self, page: &adw::TabPage) -> Option<Uuid> {
        self.pages
            .borrow()
            .iter()
            .find(|(_, p)| &p.page == page)
            .map(|(id, _)| *id)
    }

    fn connect(self: &Rc<Self>, ctrl: &Rc<Controller>) {
        let (weak, c) = (Rc::downgrade(self), ctrl.weak());
        self.view.connect_selected_page_notify(move |view| {
            let (Some(tabs), Some(c)) = (weak.upgrade(), c.upgrade()) else {
                return;
            };
            if tabs.syncing.get() {
                return;
            }
            if let Some(id) = view.selected_page().and_then(|p| tabs.id_of(&p)) {
                c.act(move |s| s.show_tab(id));
            }
        });
        let (weak, c) = (Rc::downgrade(self), ctrl.weak());
        self.view.connect_close_page(move |view, page| {
            let (Some(tabs), Some(c)) = (weak.upgrade(), c.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            if tabs.syncing.get() {
                return glib::Propagation::Proceed;
            }
            // The state decides; the page goes when the tab does.
            view.close_page_finish(page, false);
            if let Some(id) = tabs.id_of(page) {
                c.act(move |s| s.close_tab(id));
            }
            glib::Propagation::Stop
        });
        let (weak, c) = (Rc::downgrade(self), ctrl.weak());
        self.view.connect_page_reordered(move |_, page, position| {
            let (Some(tabs), Some(c)) = (weak.upgrade(), c.upgrade()) else {
                return;
            };
            if tabs.syncing.get() {
                return;
            }
            if let Some(id) = tabs.id_of(page) {
                c.act(move |s| s.move_tab(id, position.max(0) as usize));
            }
        });
        // Tabs come from the app's own menu, not the bar's "+".
        self.view.connect_create_window(|_| None);
    }

    /// The pane showing `buffer` in the current tab, else any.
    pub fn pane_showing(&self, buffer: &BufferId) -> Option<BufferPane> {
        let panes = self.panes.borrow();
        let showing = || {
            panes
                .values()
                .filter(|p| p.buffer().as_ref() == Some(buffer))
        };
        showing()
            .find(|p| p.widget().is_mapped())
            .or_else(|| showing().next())
            .cloned()
    }

    pub fn focused_pane(&self, state: &AppState) -> Option<BufferPane> {
        self.panes.borrow().get(&state.layout().focused).cloned()
    }

    pub fn refresh(
        &self,
        ctrl: &Rc<Controller>,
        state: &AppState,
        changes: &Changes,
        palette: &Palette,
    ) {
        self.syncing.set(true);
        if changes.layout || changes.structure || self.pages.borrow().is_empty() {
            self.sync_pages(ctrl, state, palette);
        }
        let current = state.current_tab();
        for (i, tab) in state.tabs().iter().enumerate() {
            let pages = self.pages.borrow();
            let Some(page) = pages.get(&tab.id) else {
                continue;
            };
            page.page.set_title(&state.tab_title(tab));
            let (unread, mentions) = state.tab_unread(tab);
            page.page
                .set_needs_attention(mentions > 0 && tab.id != current);
            page.page.set_tooltip(&match (unread, mentions) {
                (0, _) => String::new(),
                (n, 0) => format!("{n} unread"),
                (n, m) => format!("{n} unread, {m} for you"),
            });
            if self.view.page_position(&page.page) != i as i32 {
                self.view.reorder_page(&page.page, i as i32);
            }
        }
        if let Some(page) = self.pages.borrow().get(&current)
            && self.view.selected_page().as_ref() != Some(&page.page)
        {
            self.view.set_selected_page(&page.page);
        }
        self.syncing.set(false);

        // What each pane shows.
        let focused = state.layout().focused;
        for tab in state.tabs() {
            for pane in tab.layout.panes() {
                let Some(view) = self.panes.borrow().get(&pane.id).cloned() else {
                    continue;
                };
                let is_focused = tab.id == current && pane.id == focused;
                if changes.layout || changes.structure || view.buffer() != pane.buffer {
                    view.refresh(state, pane.buffer.as_ref(), is_focused);
                }
                let Some(buffer) = pane.buffer.as_ref() else {
                    continue;
                };
                let details = changes.buffers.contains(buffer);
                if details || changes.messages.contains(buffer) {
                    view.buffer_changed(state, details);
                }
            }
        }
    }

    /// Restyles every pane (theme or preferences changed).
    pub fn restyle(&self, state: &AppState, palette: &Palette) {
        for pane in self.panes.borrow().values() {
            pane.restyle(state, palette.clone());
        }
    }

    fn sync_pages(&self, ctrl: &Rc<Controller>, state: &AppState, palette: &Palette) {
        let wanted: Vec<Uuid> = state.tabs().iter().map(|t| t.id).collect();
        // Close pages for tabs that are gone.
        let gone: Vec<Uuid> = self
            .pages
            .borrow()
            .keys()
            .filter(|id| !wanted.contains(id))
            .copied()
            .collect();
        for id in gone {
            if let Some(page) = self.pages.borrow_mut().remove(&id) {
                teardown(page.host.child());
                page.host.set_child(gtk::Widget::NONE);
                self.view.close_page(&page.page);
            }
        }
        // Panes that are gone.
        let live: Vec<Uuid> = state
            .tabs()
            .iter()
            .flat_map(|t| t.layout.panes().into_iter().map(|p| p.id))
            .collect();
        self.panes.borrow_mut().retain(|id, _| live.contains(id));
        for id in &live {
            if !self.panes.borrow().contains_key(id) {
                let pane = BufferPane::new(ctrl, *id, &state.prefs, palette.clone());
                self.panes.borrow_mut().insert(*id, pane);
            }
        }
        for tab in state.tabs() {
            let shape = shape(&tab.layout.root);
            let mut pages = self.pages.borrow_mut();
            let page = pages.entry(tab.id).or_insert_with(|| {
                let host = adw::Bin::new();
                let page = self.view.append(&host);
                Page {
                    page,
                    host,
                    shape: String::new(),
                }
            });
            if page.shape != shape {
                teardown(page.host.child());
                let widget = build(&tab.layout.root, &self.panes.borrow());
                page.host.set_child(Some(&widget));
                page.shape = shape;
            }
        }
    }
}

/// The tree's structure: ids and axes, not sizes or buffers.
fn shape(node: &PaneNode) -> String {
    match node {
        PaneNode::Pane(p) => format!("p{}", p.id),
        PaneNode::Split(s) => format!(
            "s{}{:?}({})",
            s.id,
            s.axis,
            s.children.iter().map(shape).collect::<Vec<_>>().join(",")
        ),
    }
}

/// Takes pane widgets out of their old split views so they can be placed
/// again.
fn teardown(widget: Option<gtk::Widget>) {
    let Some(widget) = widget else {
        return;
    };
    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        let (start, end) = (paned.start_child(), paned.end_child());
        paned.set_start_child(gtk::Widget::NONE);
        paned.set_end_child(gtk::Widget::NONE);
        teardown(start);
        teardown(end);
    }
}

fn build(node: &PaneNode, panes: &HashMap<Uuid, BufferPane>) -> gtk::Widget {
    match node {
        PaneNode::Pane(p) => match panes.get(&p.id) {
            Some(pane) => {
                let w = pane.widget().clone();
                if let Some(parent) = w.parent() {
                    if let Some(bin) = parent.downcast_ref::<adw::Bin>() {
                        bin.set_child(gtk::Widget::NONE);
                    } else {
                        w.unparent();
                    }
                }
                w.upcast()
            }
            None => gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
        },
        PaneNode::Split(s) => {
            let orientation = match s.axis {
                SplitAxis::Horizontal => gtk::Orientation::Horizontal,
                SplitAxis::Vertical => gtk::Orientation::Vertical,
            };
            let children: Vec<gtk::Widget> = s.children.iter().map(|c| build(c, panes)).collect();
            chain(orientation, &children, &s.fractions, 0)
        }
    }
}

/// n children as nested two-way split views, sized by `fractions`.
fn chain(
    orientation: gtk::Orientation,
    children: &[gtk::Widget],
    fractions: &[f64],
    from: usize,
) -> gtk::Widget {
    if children.len() - from == 1 {
        return children[from].clone();
    }
    let paned = gtk::Paned::builder()
        .orientation(orientation)
        .wide_handle(true)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .css_classes(["pane-split"])
        .build();
    paned.set_start_child(Some(&children[from]));
    paned.set_end_child(Some(&chain(orientation, children, fractions, from + 1)));
    // This child's share of what's left from here on.
    let rest: f64 = fractions[from..].iter().sum();
    let share = if rest > 0.0 {
        fractions[from] / rest
    } else {
        0.5
    };
    let placed = Rc::new(Cell::new(false));
    let place = move |paned: &gtk::Paned| {
        if placed.get() {
            return;
        }
        let size = match orientation {
            gtk::Orientation::Horizontal => paned.width(),
            _ => paned.height(),
        };
        if size > 0 {
            placed.set(true);
            paned.set_position((f64::from(size) * share).round() as i32);
        }
    };
    paned.connect_map(move |paned| {
        let (paned, place) = (paned.clone(), place.clone());
        glib::timeout_add_local_once(Duration::from_millis(10), move || place(&paned));
    });
    paned.upcast()
}
