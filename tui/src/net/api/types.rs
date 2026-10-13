pub struct NetApi {
    addr: String,
}

impl NetApi {
    pub fn new(addr: String) -> Self {
        Self { addr }
    }

    pub async fn connect(&self) -> color_eyre::Result<()> {
        reqwest::get(format!("{}/health", self.addr))
            .await?
            .text()
            .await?;

        Ok(())
    }
}
