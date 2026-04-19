#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub id: String,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub active: bool,
    pub badge_count: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum BackendEvent {
    Snapshot(Vec<WindowState>),
    #[allow(dead_code)]
    BadgeUpdate {
        id: String,
        count: Option<u32>,
    },
}

#[derive(Clone, Debug)]
pub enum BackendRequest {
    Activate(String),
    Close(String),
}
