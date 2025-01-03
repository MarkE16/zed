use std::sync::Arc;

use fuzzy::StringMatchCandidate;
use gpui::{AppContext, DismissEvent, FocusHandle, FocusableView, Model, Task, WeakModel};
use picker::{Picker, PickerDelegate};
use ui::{prelude::*, ListItem};

use crate::context::ContextKind;
use crate::context_picker::{ConfirmBehavior, ContextPicker};
use crate::context_store;
use crate::thread::ThreadId;
use crate::thread_store::ThreadStore;

pub struct ThreadContextPicker {
    picker: Model<Picker<ThreadContextPickerDelegate>>,
}

impl ThreadContextPicker {
    pub fn new(
        thread_store: WeakModel<ThreadStore>,
        context_picker: WeakModel<ContextPicker>,
        context_store: WeakModel<context_store::ContextStore>,
        confirm_behavior: ConfirmBehavior,
        window: &mut Window,
        cx: &mut ModelContext<Self>,
    ) -> Self {
        let delegate = ThreadContextPickerDelegate::new(
            thread_store,
            context_picker,
            context_store,
            confirm_behavior,
        );
        let picker = window.new_view(cx, |window, cx| Picker::uniform_list(delegate, window, cx));

        ThreadContextPicker { picker }
    }
}

impl FocusableView for ThreadContextPicker {
    fn focus_handle(&self, cx: &AppContext) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for ThreadContextPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut ModelContext<Self>) -> impl IntoElement {
        self.picker.clone()
    }
}

#[derive(Debug, Clone)]
struct ThreadContextEntry {
    id: ThreadId,
    summary: SharedString,
}

pub struct ThreadContextPickerDelegate {
    thread_store: WeakModel<ThreadStore>,
    context_picker: WeakModel<ContextPicker>,
    context_store: WeakModel<context_store::ContextStore>,
    confirm_behavior: ConfirmBehavior,
    matches: Vec<ThreadContextEntry>,
    selected_index: usize,
}

impl ThreadContextPickerDelegate {
    pub fn new(
        thread_store: WeakModel<ThreadStore>,
        context_picker: WeakModel<ContextPicker>,
        context_store: WeakModel<context_store::ContextStore>,
        confirm_behavior: ConfirmBehavior,
    ) -> Self {
        ThreadContextPickerDelegate {
            thread_store,
            context_picker,
            context_store,
            confirm_behavior,
            matches: Vec::new(),
            selected_index: 0,
        }
    }
}

impl PickerDelegate for ThreadContextPickerDelegate {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut ModelContext<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut AppContext) -> Arc<str> {
        "Search threads…".into()
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut ModelContext<Picker<Self>>,
    ) -> Task<()> {
        let Ok(threads) = self.thread_store.update(cx, |this, cx| {
            this.threads(cx)
                .into_iter()
                .map(|thread| {
                    const DEFAULT_SUMMARY: SharedString = SharedString::new_static("New Thread");

                    let id = thread.read(cx).id().clone();
                    let summary = thread.read(cx).summary().unwrap_or(DEFAULT_SUMMARY);
                    ThreadContextEntry { id, summary }
                })
                .collect::<Vec<_>>()
        }) else {
            return Task::ready(());
        };

        let executor = cx.background_executor().clone();
        let search_task = cx.background_executor().spawn(async move {
            if query.is_empty() {
                threads
            } else {
                let candidates = threads
                    .iter()
                    .enumerate()
                    .map(|(id, thread)| StringMatchCandidate::new(id, &thread.summary))
                    .collect::<Vec<_>>();
                let matches = fuzzy::match_strings(
                    &candidates,
                    &query,
                    false,
                    100,
                    &Default::default(),
                    executor,
                )
                .await;

                matches
                    .into_iter()
                    .map(|mat| threads[mat.candidate_id].clone())
                    .collect()
            }
        });

        cx.spawn_in(window, |this, mut cx| async move {
            let matches = search_task.await;
            this.update(&mut cx, |this, cx| {
                this.delegate.matches = matches;
                this.delegate.selected_index = 0;
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut ModelContext<Picker<Self>>,
    ) {
        let Some(entry) = self.matches.get(self.selected_index) else {
            return;
        };

        let Some(thread_store) = self.thread_store.upgrade() else {
            return;
        };

        let Some(thread) = thread_store.update(cx, |this, cx| this.open_thread(&entry.id, cx))
        else {
            return;
        };

        self.context_store
            .update(cx, |context_store, cx| {
                let text = thread.update(cx, |thread, _cx| {
                    let mut text = String::new();

                    for message in thread.messages() {
                        text.push_str(match message.role {
                            language_model::Role::User => "User:",
                            language_model::Role::Assistant => "Assistant:",
                            language_model::Role::System => "System:",
                        });
                        text.push('\n');

                        text.push_str(&message.text);
                        text.push('\n');
                    }

                    text
                });

                context_store.insert_context(ContextKind::Thread, entry.summary.clone(), text);
            })
            .ok();

        match self.confirm_behavior {
            ConfirmBehavior::KeepOpen => {}
            ConfirmBehavior::Close => self.dismissed(window, cx),
        }
    }

    fn dismissed(&mut self, window: &mut Window, cx: &mut ModelContext<Picker<Self>>) {
        self.context_picker
            .update(cx, |this, cx| {
                this.reset_mode();
                cx.emit(DismissEvent);
            })
            .ok();
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut ModelContext<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let thread = &self.matches[ix];

        Some(
            ListItem::new(ix)
                .inset(true)
                .toggle_state(selected)
                .child(Label::new(thread.summary.clone())),
        )
    }
}
