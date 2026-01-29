/// Manage Chrome/Chromium instances with CDP.
pub struct BrowserManager {
    // TODO: chromiumoxide or headless_chrome pool
}

impl Default for BrowserManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn screenshot(&self, _url: &str) -> anyhow::Result<Vec<u8>> {
        todo!("navigate to URL, take screenshot via CDP")
    }
}
