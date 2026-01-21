use crate::{Xfwl4State, backend::Backend, ui::FromUiMessage};

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn handle_ui_thread_message(&mut self, message: FromUiMessage) -> anyhow::Result<()> {
        match message {
            FromUiMessage::DefaultMainContextClaimed => Ok(()),
            FromUiMessage::TabwinAction(_action) => Ok(()),
            FromUiMessage::WindowMenuAction(_action) => Ok(()),
        }
    }
}
