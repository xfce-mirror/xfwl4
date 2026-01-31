use smithay::{reexports::wayland_server::Resource, wayland::seat::WaylandFocus};

use crate::{
    Xfwl4State,
    backend::Backend,
    shell::WindowElement,
    ui::{FromUiMessage, tabwin::TabwinAction},
};

impl<BackendData: Backend + 'static> Xfwl4State<BackendData> {
    pub fn handle_ui_thread_message(&mut self, message: FromUiMessage) -> anyhow::Result<()> {
        match message {
            FromUiMessage::DefaultMainContextClaimed => Ok(()),
            FromUiMessage::IconThemeChanged(_icon_theme) => {
                //self.icon_theme.set_custom_theme(Some(&icon_theme));
                Ok(())
            }
            FromUiMessage::TabwinAction(TabwinAction::HoverWindow(_)) => Ok(()),
            FromUiMessage::TabwinAction(TabwinAction::WindowSelected(selected)) => {
                let predicate = |elem: &WindowElement| elem.0.wl_surface().is_some_and(|surf| surf.id() == selected);

                if let Some(window) = self.workspace_manager.active_workspace().find_element(predicate) {
                    let workspace = self.workspace_manager.active_workspace_mut();
                    workspace.raise_element(&window, true);
                } else {
                    let mut idx_and_window = None::<(u32, WindowElement)>;
                    for (idx, workspace) in self.workspace_manager.workspaces().iter().enumerate() {
                        if let Some(window) = workspace.find_element(predicate) {
                            idx_and_window = Some((idx as u32, window));
                            break;
                        }
                    }

                    if let Some((idx, window)) = idx_and_window {
                        self.workspace_manager.set_active_workspace(idx);
                        if let Some(workspace) = self.workspace_manager.workspaces_mut().get_mut(idx as usize) {
                            workspace.raise_element(&window, true);
                        }
                    }
                }

                Ok(())
            }
            FromUiMessage::WindowMenuAction(_action) => Ok(()),
        }
    }
}
