use smithay::{
    delegate_idle_notify,
    wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState},
};

use crate::{Xfwl4State, backend::Backend};

impl<BackendData: Backend + 'static> IdleNotifierHandler for Xfwl4State<BackendData> {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.ext_idle_notifier_state
    }
}

delegate_idle_notify!(@<BackendData: Backend + 'static> Xfwl4State<BackendData>);
