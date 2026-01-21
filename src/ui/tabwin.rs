#[derive(Debug)]
pub enum TabwinAction {
    Foo,
}

#[derive(Debug)]
pub struct TabwinWindow {
    // cairo::Surface and gdk_pixbuf::Pixbuf cannot be made Send, so send the raw bytes
    pub icon_bytes: Vec<u8>,
    pub name: String,
}
