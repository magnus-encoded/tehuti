//! Pure window membership and focus decisions.

#[derive(Debug, Clone, Default)]
pub struct Windows<Window> {
    mapped: Vec<Window>,
    focused: Option<Window>,
}

#[derive(Debug, Clone)]
pub enum Input<Window> {
    Map(Window),
    Unmap(Window),
    Focus(Window),
    Close,
    Launch {
        desktop_entry: String,
        installed: Vec<String>,
    },
    Notify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect<Window> {
    Close(Window),
    Launch,
    Notice,
}

impl<Window: Clone + Eq> Windows<Window> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mapped: Vec::new(),
            focused: None,
        }
    }

    #[must_use]
    pub const fn focus(&self) -> Option<&Window> {
        self.focused.as_ref()
    }

    #[must_use]
    pub fn contains(&self, window: &Window) -> bool {
        self.mapped.contains(window)
    }

    pub fn step(&mut self, input: Input<Window>) -> Option<Effect<Window>> {
        match input {
            Input::Map(window) => {
                if !self.mapped.contains(&window) {
                    self.mapped.push(window.clone());
                }
                self.focused = Some(window);
            }
            Input::Unmap(window) => self.remove(&window),
            Input::Focus(window) => {
                if self.mapped.contains(&window) {
                    self.focused = Some(window);
                }
            }
            Input::Close => {
                if let Some(window) = self.focused.clone() {
                    self.remove(&window);
                    return Some(Effect::Close(window));
                }
            }
            Input::Launch {
                desktop_entry,
                installed,
            } => {
                return Some(if installed.contains(&desktop_entry) {
                    Effect::Launch
                } else {
                    Effect::Notice
                });
            }
            Input::Notify => return Some(Effect::Notice),
        }
        None
    }

    fn remove(&mut self, window: &Window) {
        self.mapped.retain(|mapped| mapped != window);
        if self.focused.as_ref() == Some(window) {
            self.focused = self.mapped.last().cloned();
        }
    }
}
