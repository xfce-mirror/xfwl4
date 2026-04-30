use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};

use crate::{backend::Backend, core::state::Xfwl4State};

impl<BackendData: Backend + 'static> IdleNotifierHandler for Xfwl4State<BackendData> {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.core.protocol_delegates.ext_idle_notifier_state
    }
}
