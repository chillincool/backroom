//! Link previews: resting the mouse on a web link pops up a card with the
//! page's title, description and image (fetched by the core, only then:
//! fetching a link tells its site your address). Kept for the session.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use backroom_ffi::{LinkPreview, PreviewKind};
use gtk::{gdk, glib};

use crate::desktop::controller::Controller;

const DELAY: Duration = Duration::from_millis(600);
const LIMIT: usize = 500;

#[derive(Clone)]
enum Entry {
    Loading,
    Loaded(Rc<LinkPreview>),
    Failed,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    order: VecDeque<String>,
    /// Cards waiting for a preview, by URL.
    waiting: HashMap<String, Vec<Weak<gtk::Popover>>>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

/// Shows previews for the web links in `label`.
pub fn attach(label: &gtk::Label, ctrl: Weak<Controller>) {
    let state: Rc<RefCell<Hover>> = Rc::default();
    let motion = gtk::EventControllerMotion::new();
    let (s, l) = (state.clone(), label.downgrade());
    motion.connect_motion(move |_, x, y| {
        let Some(label) = l.upgrade() else {
            return;
        };
        let uri = label
            .current_uri()
            .map(|u| u.to_string())
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
        let mut hover = s.borrow_mut();
        if hover.uri == uri {
            return;
        }
        hover.cancel();
        hover.uri.clone_from(&uri);
        let Some(uri) = uri else {
            return;
        };
        let (s2, l2, c2) = (s.clone(), label.downgrade(), ctrl.clone());
        hover.timer = Some(glib::timeout_add_local_once(DELAY, move || {
            s2.borrow_mut().timer = None;
            if let (Some(label), Some(ctrl)) = (l2.upgrade(), c2.upgrade()) {
                let popover = show(&label, &ctrl, &uri, x, y);
                s2.borrow_mut().popover = Some(popover);
            }
        }));
    });
    let s = state.clone();
    motion.connect_leave(move |_| {
        let mut hover = s.borrow_mut();
        hover.cancel();
        hover.uri = None;
    });
    label.add_controller(motion);
    label.connect_destroy(move |_| state.borrow_mut().cancel());
}

#[derive(Default)]
struct Hover {
    uri: Option<String>,
    timer: Option<glib::SourceId>,
    popover: Option<Rc<gtk::Popover>>,
}

impl Hover {
    fn cancel(&mut self) {
        if let Some(t) = self.timer.take() {
            t.remove();
        }
        if let Some(p) = self.popover.take() {
            p.popdown();
            p.unparent();
        }
    }
}

fn show(label: &gtk::Label, ctrl: &Rc<Controller>, uri: &str, x: f64, y: f64) -> Rc<gtk::Popover> {
    let popover = Rc::new(
        gtk::Popover::builder()
            .autohide(false)
            .has_arrow(true)
            .position(gtk::PositionType::Top)
            .can_focus(false)
            .build(),
    );
    popover.set_parent(label);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let entry = CACHE.with(|c| c.borrow().entries.get(uri).cloned());
    match entry {
        Some(entry) => fill(&popover, uri, &entry),
        None => {
            fill(&popover, uri, &Entry::Loading);
            fetch(ctrl, uri);
        }
    }
    if matches!(
        CACHE.with(|c| c.borrow().entries.get(uri).cloned()),
        Some(Entry::Loading)
    ) {
        CACHE.with(|c| {
            c.borrow_mut()
                .waiting
                .entry(uri.to_owned())
                .or_default()
                .push(Rc::downgrade(&popover));
        });
    }
    popover.popup();
    popover
}

fn fetch(ctrl: &Rc<Controller>, uri: &str) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.entries.insert(uri.to_owned(), Entry::Loading);
        c.order.push_back(uri.to_owned());
        if c.order.len() > LIMIT
            && let Some(old) = c.order.pop_front()
        {
            c.entries.remove(&old);
        }
    });
    let Some(previewer) = ctrl.previewer.clone() else {
        finish(uri, Entry::Failed);
        return;
    };
    let uri = uri.to_owned();
    glib::spawn_future_local(async move {
        let entry = match previewer.preview(uri.clone()).await {
            Some(p) => Entry::Loaded(Rc::new(p)),
            None => Entry::Failed,
        };
        finish(&uri, entry);
    });
}

fn finish(uri: &str, entry: Entry) {
    let waiting = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.entries.contains_key(uri) {
            c.entries.insert(uri.to_owned(), entry.clone());
        }
        c.waiting.remove(uri).unwrap_or_default()
    });
    for p in waiting.iter().filter_map(Weak::upgrade) {
        fill(&p, uri, &entry);
    }
}

fn host(uri: &str) -> String {
    uri.split_once("://")
        .map(|(_, r)| r.split(['/', '?', '#']).next().unwrap_or(r))
        .unwrap_or(uri)
        .trim_start_matches("www.")
        .to_owned()
}

fn fill(popover: &gtk::Popover, uri: &str, entry: &Entry) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("preview-card");
    card.set_width_request(320);
    match entry {
        Entry::Loading => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&gtk::Spinner::builder().spinning(true).build());
            row.append(&gtk::Label::new(Some(&host(uri))));
            card.append(&row);
        }
        Entry::Failed => {
            let l = gtk::Label::builder()
                .label(format!("{}\nNo preview", host(uri)))
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build();
            card.append(&l);
        }
        Entry::Loaded(p) => {
            if let Some(image) = &p.image
                && let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(image.as_slice()))
            {
                let picture = gtk::Picture::for_paintable(&texture);
                picture.set_can_shrink(true);
                picture.set_content_fit(gtk::ContentFit::Contain);
                let max = if p.kind == PreviewKind::Image {
                    320
                } else {
                    180
                };
                let (w, h) = (texture.width().max(1), texture.height().max(1));
                let height = (320 * h / w).min(max);
                picture.set_size_request(320, height);
                card.append(&picture);
            }
            let site = p.site_name.clone().unwrap_or_else(|| host(&p.url));
            card.append(
                &gtk::Label::builder()
                    .label(&site)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["caption", "dim-label"])
                    .build(),
            );
            if let Some(title) = &p.title {
                card.append(
                    &gtk::Label::builder()
                        .label(title)
                        .xalign(0.0)
                        .wrap(true)
                        .lines(2)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .max_width_chars(40)
                        .css_classes(["heading"])
                        .build(),
                );
            }
            if let Some(description) = &p.description {
                card.append(
                    &gtk::Label::builder()
                        .label(description)
                        .xalign(0.0)
                        .wrap(true)
                        .lines(3)
                        .ellipsize(gtk::pango::EllipsizeMode::End)
                        .max_width_chars(40)
                        .build(),
                );
            }
        }
    }
    popover.set_child(Some(&card));
}
