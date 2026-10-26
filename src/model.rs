#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub id: String,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub active: bool,
    pub badge_count: Option<u32>,
    /// Connector names for this window; `None` means unknown, an empty list means no output.
    pub outputs: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub enum BackendRequest {
    Activate(String),
    Close(String),
}
