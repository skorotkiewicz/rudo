#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub id: String,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub enum BackendEvent {
    Snapshot(Vec<WindowState>),
}

#[derive(Clone, Debug)]
pub enum BackendRequest {
    Activate(String),
    Close(String),
}
