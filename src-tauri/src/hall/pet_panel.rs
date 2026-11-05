//! Window-instance-scoped pet panel lifetime. No agent or permission changes.

pub struct PanelState {
    instance: Option<String>,
    pinned: bool,
    shown: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        Self::new()
    }
}

impl PanelState {
    pub const fn new() -> Self {
        Self {
            instance: None,
            pinned: true,
            shown: false,
        }
    }

    pub fn open(&mut self, instance: String) {
        self.instance = Some(instance);
        // Keep the window safe before its remote permissions/drafts load.
        self.pinned = true;
        self.shown = false;
    }

    pub fn pinned(&self, instance: &str) -> Option<bool> {
        (self.instance.as_deref() == Some(instance)).then_some(self.pinned)
    }

    pub fn set_pinned(&mut self, instance: &str, pinned: bool) -> bool {
        if self.pinned(instance).is_none() {
            return false;
        }
        self.pinned = pinned;
        true
    }

    pub fn dismiss_on_blur(&self, instance: &str) -> bool {
        self.pinned(instance) == Some(false)
    }

    /// Only the first layout may show/focus the initially hidden OS window.
    pub fn show_on_resize(&mut self, instance: &str) -> bool {
        if self.pinned(instance).is_none() || self.shown {
            return false;
        }
        self.shown = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::PanelState;

    #[test]
    fn loading_and_pinned_panels_survive_blur_but_unpinned_panels_dismiss() {
        let mut state = PanelState::new();
        state.open("remote-a".into());
        assert!(!state.dismiss_on_blur("remote-a"));
        assert!(state.set_pinned("remote-a", false));
        assert!(state.dismiss_on_blur("remote-a"));
        assert!(state.set_pinned("remote-a", true));
        assert!(!state.dismiss_on_blur("remote-a"));
    }

    #[test]
    fn late_old_panel_updates_cannot_unpin_or_dismiss_a_new_backend() {
        let mut state = PanelState::new();
        state.open("remote-a".into());
        state.set_pinned("remote-a", false);
        state.open("remote-b".into());
        assert!(!state.set_pinned("remote-a", false));
        assert!(!state.dismiss_on_blur("remote-a"));
        assert_eq!(state.pinned("remote-b"), Some(true));
        assert!(!state.dismiss_on_blur("remote-b"));
    }

    #[test]
    fn resize_after_first_show_never_steals_focus_and_reopen_resets_lifetime() {
        let mut state = PanelState::new();
        state.open("local-first".into());
        assert!(state.show_on_resize("local-first"));
        assert!(!state.show_on_resize("local-first"));
        state.open("local-next".into());
        assert!(!state.show_on_resize("local-first"));
        assert!(state.show_on_resize("local-next"));
        assert!(!state.show_on_resize("local-next"));
    }
}
