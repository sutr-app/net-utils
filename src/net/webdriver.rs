use anyhow::{anyhow, Context};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, FixedOffset};
use command_utils::util::datetime;
use deadpool::managed::{
    Manager, Metrics, Object, Pool, PoolConfig, PoolError, RecycleError, RecycleResult, Timeouts,
};
use deadpool::Runtime;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationSeconds;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;
use strum::{self, IntoEnumIterator};
use strum_macros::{self, EnumIter};
use thirtyfour::prelude::*;
use thirtyfour::session::http::HttpClient;
use thirtyfour::ChromeCapabilities;

use super::reqwest::ReqwestClient;

#[serde_as]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WebDriverConfig {
    pub url: Option<String>,
    pub user_agent: Option<String>,
    #[serde_as(as = "Option<DurationSeconds<f64>>")]
    pub page_load_timeout_sec: Option<Duration>,
    #[serde_as(as = "Option<DurationSeconds<f64>>")]
    pub script_timeout_sec: Option<Duration>,
    pub pool_max_size: usize,
    #[serde_as(as = "Option<DurationSeconds<f64>>")]
    pub pool_timeout_wait_sec: Option<Duration>,
    #[serde_as(as = "Option<DurationSeconds<f64>>")]
    pub pool_timeout_create_sec: Option<Duration>,
    #[serde_as(as = "Option<DurationSeconds<f64>>")]
    pub pool_timeout_recycle_sec: Option<Duration>,
}

impl Default for WebDriverConfig {
    fn default() -> Self {
        Self {
            url: None,
            user_agent: None,
            page_load_timeout_sec: Some(Duration::from_secs(30)),
            script_timeout_sec: Some(Duration::from_secs(30)),
            pool_max_size: 3, // Production recommended: limit concurrent sessions
            pool_timeout_wait_sec: Some(Duration::from_secs(120)), // 2 minutes wait
            pool_timeout_create_sec: Some(Duration::from_secs(60)), // 1 minute creation
            pool_timeout_recycle_sec: Some(Duration::from_secs(30)), // 30 seconds recycle
        }
    }
}

pub trait UsePoolConfig {
    fn pool_config(&self) -> PoolConfig;
}

impl UsePoolConfig for WebDriverConfig {
    fn pool_config(&self) -> PoolConfig {
        PoolConfig {
            max_size: self.pool_max_size,
            timeouts: Timeouts {
                wait: self.pool_timeout_wait_sec,
                create: self.pool_timeout_create_sec,
                recycle: self.pool_timeout_recycle_sec,
            },
            queue_mode: deadpool::managed::QueueMode::Fifo,
        }
    }
}
#[derive(Debug, Clone)]
pub struct ChromeDriverFactory {
    pub config: WebDriverConfig,
    pub capabilities: ChromeCapabilities,
}

pub trait UseWebDriver {
    fn driver(&self) -> &WebDriver;
}
pub struct WebDriverWrapper {
    driver: WebDriver,
    created_at: std::time::Instant,
}

impl UseWebDriver for WebDriverWrapper {
    fn driver(&self) -> &WebDriver {
        &self.driver
    }
}

impl WebDriverWrapper {
    pub async fn is_session_healthy(&self) -> bool {
        // Test session with lightweight WebDriver command
        // If session is invalid, this will fail
        self.driver.window().await.is_ok()
    }

    pub fn created_at(&self) -> std::time::Instant {
        self.created_at
    }
}

impl ChromeDriverFactory {
    // default value (for test)
    // mac chrome - 最新のChrome版に更新してより自然に
    const USER_AGENT: &'static str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";
    const TIME_OUT: Duration = Duration::from_secs(5);
    const URL: &'static str = "http://localhost:9515";

    fn build_chrome_capabilities(
        user_agent: impl Into<Option<String>> + Send,
    ) -> Result<ChromeCapabilities, Box<WebDriverError>> {
        let mut caps = DesiredCapabilities::chrome();
        // https://stackoverflow.com/a/52340526
        // https://stackoverflow.com/questions/48450594/selenium-timed-out-receiving-message-from-renderer
        // caps.add_extension(Path::new("./adblock.crx"))?;
        // caps.add_arg("--window-size=1920,1080")?;
        caps.add_arg("start-maximized")?; // open Browser in maximized mode
        caps.add_arg("--headless=new")?;
        caps.add_arg("--no-sandbox")?; // Bypass OS security model // necessary in docker env
        caps.add_arg("--disable-dev-shm-usage")?; // if tab crash error occurred (add shm mem to container or this option turned on)
        caps.add_arg("--disable-browser-side-navigation")?; //https://stackoverflow.com/a/49123152/1689770"
        caps.add_arg("--disable-gpu")?;

        // Renderer timeout対策
        caps.add_arg("--disable-web-security")?; // セキュリティチェックを無効化（テスト環境用）
        caps.add_arg("--disable-features=TranslateUI")?; // 翻訳機能を無効化
        caps.add_arg("--disable-ipc-flooding-protection")?; // IPCフラッディング保護を無効化
        caps.add_arg("--disable-renderer-backgrounding")?; // バックグラウンドレンダラーを無効化
        caps.add_arg("--disable-backgrounding-occluded-windows")?; // オクルージョンによるバックグラウンド化を無効化
        caps.add_arg("--disable-field-trial-config")?; // フィールドトライアル設定を無効化
        caps.add_arg("--disable-back-forward-cache")?; // バックフォワードキャッシュを無効化
        caps.add_arg("--disable-component-extensions-with-background-pages")?; // バックグラウンドページ付きコンポーネント拡張を無効化

        // JavaScript実行の最適化
        caps.add_arg("--max_old_space_size=4096")?; // V8メモリサイズを増加（パフォーマンス最適化のみ）
        caps.add_arg("--js-flags=--max-old-space-size=4096")?; // JavaScriptエンジンのメモリ上限を増加（パフォーマンス最適化のみ）

        // ネットワーク関連の最適化
        caps.add_arg("--aggressive-cache-discard")?; // キャッシュを積極的に破棄
        caps.add_arg("--disable-background-networking")?; // バックグラウンドネットワーキングを無効化
        caps.add_arg("--disable-default-apps")?; // デフォルトアプリを無効化
        caps.add_arg("--disable-sync")?; // 同期機能を無効化

        // レンダリングプロセスの最適化
        // --single-processは一部の環境で問題を起こす可能性があるため削除
        // caps.add_arg("--single-process")?;
        // caps.add_arg("--disable-plugins")?; // プラグインを無効化（パフォーマンス最適化）
        // caps.add_arg("--disable-images")?; // 画像読み込みを無効化（パフォーマンス最適化）

        // 追加の最適化オプション
        caps.add_arg("--disable-logging")?; // ログ出力を無効化
        caps.add_arg("--disable-metrics")?; // メトリクス収集を無効化
        caps.add_arg("--disable-metrics-repo")?; // メトリクスレポートを無効化
        caps.add_arg("--mute-audio")?; // 音声を無効化
        caps.add_arg("--no-default-browser-check")?; // デフォルトブラウザチェックを無効化

        caps.add_arg("--disable-hang-monitor")?; // ハングモニターを無効化
        caps.add_arg("--disable-prompt-on-repost")?; // 再送信時のプロンプトを無効化
        caps.add_arg("--disable-translate")?; // 翻訳機能を無効化
        caps.add_arg("--disable-search-engine-choice-screen")?; // 検索エンジン選択画面を無効化
        caps.add_arg("--use-mock-keychain")?; // モックキーチェーンを使用
        caps.add_arg("--disable-component-update")?; // コンポーネント更新を無効化

        caps.add_arg("--disable-infobars")?; // disabling infobars
        caps.add_arg("--disable-extensions")?;
        caps.add_arg("--dns-prefetch-disable")?;

        // https://stackoverflow.com/questions/53039551/selenium-webdriver-modifying-navigator-webdriver-flag-to-prevent-selenium-detec
        caps.add_arg("--disable-blink-features=AutomationControlled")?;
        caps.add_arg("--disable-browser-side-navigation")?;

        caps.add_exclude_switch("enable-automation")?;
        // caps.add_experimental_option("useAutomationExtension", false)?; // deprecated - 削除

        let prefs = serde_json::json!({
            "credentials_enable_service": false,
            "profile.password_manager_enabled": false,
            "profile.default_content_setting_values.notifications": 2,
            "profile.default_content_settings.popups": 0,
            "profile.managed_default_content_settings.images": 2,
            // WebRTC
            "webrtc.ip_handling_policy": "disable_non_proxied_udp",
            "webrtc.multiple_routes_enabled": false,
            "webrtc.nonproxied_udp_enabled": false,
            // メディア関連の設定
            "profile.default_content_setting_values.media_stream_mic": 2,
            "profile.default_content_setting_values.media_stream_camera": 2,
            "profile.default_content_setting_values.geolocation": 2,
            // プライバシー
            "profile.default_content_setting_values.plugins": 2,
            "profile.default_content_setting_values.ppapi_broker": 2,
            "profile.default_content_setting_values.automatic_downloads": 2
        });
        caps.add_experimental_option("prefs", prefs)?;

        caps.add_arg(
            format!(
                "--user-agent={}",
                user_agent
                    .into()
                    .unwrap_or_else(|| Self::USER_AGENT.to_string())
            )
            .as_str(),
        )?;
        caps.add_arg("--lang=ja-JP")?;
        Ok(caps)
    }

    pub async fn new(
        config: WebDriverConfig,
        // server_url: impl Into<String> + Sync,
        // page_load_timeout: Duration,
        // script_timeout: Duration,
        // user_agent: impl Into<String> + Sync,
    ) -> Result<Self, Box<WebDriverError>> {
        let caps = Self::build_chrome_capabilities(config.user_agent.clone())?;

        Ok(Self {
            config,
            capabilities: caps,
        })
    }

    pub async fn create(&self) -> Result<WebDriverWrapper, WebDriverError> {
        let driver = WebDriver::new(
            self.config.url.as_deref().unwrap_or(Self::URL),
            self.capabilities.clone(),
        )
        .await?;

        // タイムアウト設定の詳細ログ
        let page_load_timeout = self.config.page_load_timeout_sec.unwrap_or(Self::TIME_OUT);
        let script_timeout = self.config.script_timeout_sec.unwrap_or(Self::TIME_OUT);

        tracing::info!(
            "Setting WebDriver timeouts - page_load: {:?}, script: {:?}",
            page_load_timeout,
            script_timeout
        );

        driver.set_page_load_timeout(page_load_timeout).await?;
        // driver.set_implicit_wait_timeout(page_load_timeout).await?;
        driver.set_script_timeout(script_timeout).await?;

        // タイムアウト設定が正しく適用されたかを確認
        tracing::info!("WebDriver timeouts configured successfully");

        Ok(WebDriverWrapper {
            driver,
            created_at: std::time::Instant::now(),
        })
    }
}

#[derive(Debug)]
pub struct WebDriverManagerImpl {
    web_driver_factory: ChromeDriverFactory,
    // connection_counter: AtomicUsize,
}

impl WebDriverManagerImpl {
    pub async fn new(config: WebDriverConfig) -> Result<Self, Box<WebDriverError>> {
        let driver = ChromeDriverFactory::new(config).await?;
        Ok(Self {
            web_driver_factory: driver,
            // connection_counter: AtomicUsize::new(0),
        })
    }
}

impl Manager for WebDriverManagerImpl {
    type Type = WebDriverWrapper;
    type Error = WebDriverError;

    async fn create(&self) -> Result<WebDriverWrapper, WebDriverError> {
        // Retry mechanism for WebDriver creation to handle connection issues
        const MAX_RETRIES: u32 = 3;
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match self.web_driver_factory.create().await {
                Ok(driver) => {
                    if attempt > 1 {
                        tracing::info!("WebDriver creation succeeded on attempt {}", attempt);
                    }
                    return Ok(driver);
                }
                Err(e) => {
                    tracing::warn!("WebDriver creation attempt {} failed: {:?}", attempt, e);
                    last_error = Some(e);

                    if attempt < MAX_RETRIES {
                        // Exponential backoff: 2^(attempt-1) seconds
                        let delay_secs = 2_u64.pow(attempt - 1);
                        tracing::info!("Retrying WebDriver creation in {} seconds...", delay_secs);
                        tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                    }
                }
            }
        }

        tracing::error!("WebDriver creation failed after {} attempts", MAX_RETRIES);
        Err(last_error.unwrap())
    }

    async fn recycle(
        &self,
        wrap: &mut WebDriverWrapper,
        _metrics: &Metrics,
    ) -> RecycleResult<WebDriverError> {
        // Use improved session health check instead of title() method
        if wrap.is_session_healthy().await {
            tracing::debug!("webdriver recycle (reuse): session is healthy");
            Ok(())
        } else {
            tracing::info!("webdriver recycle error: session is not healthy, quitting instance");
            let quit = wrap.quit().await;
            Err(RecycleError::Message(Cow::Owned(format!(
                "session is not healthy, quit: {quit:?}"
            ))))
        }
    }

    fn detach(&self, obj: &mut WebDriverWrapper) {
        tracing::info!("webdriver detached");
        let driver = obj.driver.clone();

        // 現在のTokioランタイムが利用可能な場合のみquit処理を実行
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                // quit処理にタイムアウトを設定して、無期限に待機することを防ぐ
                let quit_result =
                    tokio::time::timeout(Duration::from_secs(10), driver.quit()).await;
                match quit_result {
                    Ok(Ok(())) => {
                        tracing::debug!("WebDriver quit successfully");
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("WebDriver quit failed: {:?}", e);
                    }
                    Err(_) => {
                        tracing::warn!("WebDriver quit timeout after 10 seconds");
                    }
                }
            });
        } else {
            // ランタイムが利用できない場合は警告のみ出力
            tracing::warn!(
                "Tokio runtime not available, WebDriver session may not be properly closed"
            );
        }
    }
}
pub trait UseWebDriverPool {
    fn web_driver_pool(&self) -> &WebDriverPool;
}

pub type WebDriverWrapperError = WebDriverError;
// pub type WebDriverPool = Pool<WebDriverManagerImpl, Object<WebDriverManagerImpl>>;
pub type WebDriverPoolError = PoolError<WebDriverWrapperError>;

pub struct WebDriverPool {
    pub pool: Pool<WebDriverManagerImpl, Object<WebDriverManagerImpl>>,
    pub max_size: usize,
    pub user_agent: Option<String>,
    pub config: WebDriverConfig,
}
impl WebDriverPool {
    pub async fn new(wd_config: WebDriverConfig) -> Self {
        let manager = WebDriverManagerImpl::new(wd_config.clone()).await.unwrap();
        WebDriverPool {
            pool: Pool::builder(manager)
                .config(wd_config.pool_config())
                .runtime(Runtime::Tokio1)
                .build()
                .unwrap(),
            max_size: wd_config.pool_max_size,
            user_agent: wd_config.user_agent.clone(),
            config: wd_config,
        }
    }
    pub async fn get(&self) -> Result<Object<WebDriverManagerImpl>, WebDriverPoolError> {
        self.pool.get().await
    }

    pub async fn get_with_retry(
        &self,
        max_retries: usize,
    ) -> Result<Object<WebDriverManagerImpl>, WebDriverPoolError> {
        let mut last_error = None;

        for attempt in 1..=max_retries {
            match self.pool.get().await {
                Ok(obj) => {
                    if attempt > 1 {
                        tracing::info!("WebDriver pool get succeeded on attempt {}", attempt);
                    }
                    return Ok(obj);
                }
                Err(e) => {
                    tracing::warn!("WebDriver pool get attempt {} failed: {:?}", attempt, e);
                    last_error = Some(e);

                    if attempt < max_retries {
                        let delay_secs = 2;
                        tracing::info!("Retrying pool get in {} seconds...", delay_secs);
                        tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                    }
                }
            }
        }

        tracing::error!("WebDriver pool get failed after {} attempts", max_retries);
        Err(last_error.unwrap())
    }
}

#[async_trait]
pub trait WebScraper: UseWebDriver + Send + Sync {
    const MAX_PAGE_NUM: i32 = 50;
    // catch scraping error and capture screenshot
    // TODO Unsafe境界のエラーで入れられていない...
    // async fn try_scraping<T, F>(&self, process: F) -> Result<T, WebDriverError>
    // where
    //     Self: Sized + UnwindSafe,
    //     T: Send,
    //     F: Future<Output = Result<T, WebDriverError>> + Send + UnwindSafe,
    // {
    //     match process.catch_unwind().await {
    //         Ok(r) => Ok(r),
    //         Err(e) => {
    //             self.driver()
    //                 .screenshot(Path::new("./error_browser_screen.png"))
    //                 .await?;
    //             tracing::error!("caught panic: {:?}", e);
    //             Err(anyhow!("error in parsing datetime: {:?}", e))
    //         }
    //     }
    //     .flatten()
    // }
    #[allow(clippy::too_many_arguments)]
    async fn scraping(
        &self,
        url: impl Into<String> + Send,
        title_selector: By,
        content_selector: By,
        datetime_selector: Option<By>,
        datetime_attribute: &Option<String>,
        datetime_regex: &Option<String>,
        next_page_selector: Option<By>,
        tags_selector: Option<By>,
    ) -> Result<ScrapedData, WebDriverError> {
        let u: String = url.into();
        // ignore error for scraping (ignore not respond resource)
        if let Err(err) = self.driver().goto(&u).await {
            tracing::warn!("loading error: {:?}", err)
        }
        // self.driver().screenshot(path::Path::new("./browser_screen.png")).await?;
        //  impressはsourceを取るとscraping可能になったので念のためやっておく...
        let source = self.driver().source().await?;
        tracing::trace!("page source:{:?}", source);

        // wait title element
        let elem = self.driver().query(title_selector.clone()).first().await?;
        elem.wait_until().displayed().await?;

        let title_ele = self.driver().find(title_selector).await.map_err(|e| {
            tracing::warn!("error in scraping title: {:?}", &e);
            WebDriverError::ParseError(format!(
                "error in scraping page title: {:?}, err: {:?}",
                &u, e
            ))
        })?;
        let title = title_ele.text().await?;
        tracing::info!("title: {} ", &title);

        // parse contents
        let contents = self
            .scrape_content(&content_selector, next_page_selector.as_ref())
            .await
            .inspect_err(|e| tracing::warn!("error in scraping contents: {:?}", &e))?;
        if contents.is_empty() {
            Err(WebDriverError::ParseError(format!(
                "content not found: {u}"
            )))? // bail out
        }

        // parse datetime ()
        let datetime = if let Some(sel) = datetime_selector {
            self.parse_datetime(sel, datetime_attribute, datetime_regex)
                .await
                .map_err(|e| {
                    tracing::warn!("error in parse_datetime: {:?}", &e);
                    WebDriverError::ParseError(format!("parse_datetime error: {e:?}"))
                })
                .unwrap_or(None)
        } else {
            None
        };
        tracing::info!("datetime res: {:?}", &datetime);

        let tags = if let Some(tsel) = tags_selector {
            let tag_elems = self
                .driver()
                .find_all(tsel)
                .await
                .inspect_err(|e| tracing::warn!("error in scraping tags: {:?}", &e))
                .unwrap_or_default();

            let mut tags = Vec::new();
            for elem in tag_elems.iter() {
                elem.wait_until().displayed().await.unwrap_or_default();
                let tag = elem.text().await.unwrap_or_default();
                tags.push(tag);
            }
            tags
        } else {
            vec![]
        };
        tracing::info!("content: len={} ", &contents.iter().len());
        Ok(ScrapedData {
            title,
            content: contents,
            datetime,
            tags,
        })
        // match process.catch_unwind().await {
        //     Ok(r) => Ok(r),
        //     Err(e) => {
        //         self.driver()
        //             .screenshot(Path::new("./error_browser_screen.png"))
        //             .await?;
        //         tracing::error!("caught panic: {:?}", e);
        //         Err(anyhow!("error in parsing datetime: {:?}", e))
        //     }
        // }
        // .flatten()
    }

    #[allow(unstable_name_collisions)] // for flatten()
    async fn parse_datetime(
        &self,
        datetime_selector: By,
        datetime_attribute: &Option<String>,
        datetime_regex: &Option<String>,
    ) -> Result<Option<DateTime<FixedOffset>>, anyhow::Error> {
        let datetime_element = self
            .driver()
            .find(datetime_selector)
            .await
            .inspect_err(|e| tracing::warn!("error in scraping datetime: {:?}", &e))?;
        let dt_value = match datetime_attribute {
            Some(dta) => datetime_element.attr(dta).await?.unwrap_or_default(),
            None => datetime_element.text().await?,
        };
        tracing::info!("scraped datetime: {:?}", &dt_value);
        std::panic::catch_unwind(move || {
            let dt = match datetime_regex {
                Some(fmt) if !fmt.is_empty() => {
                    let now = datetime::now();
                    Regex::new(fmt.as_str())
                        .map(|dt_re| {
                            dt_re.captures(dt_value.as_str()).and_then(|c| {
                                // XXX 年月日、時分秒の順でマッチする前提
                                datetime::ymdhms(
                                    c.get(1).map_or(now.year(), |r| r.as_str().parse().unwrap()),
                                    c.get(2).map_or(0u32, |r| r.as_str().parse().unwrap()),
                                    c.get(3).map_or(0u32, |r| r.as_str().parse().unwrap()),
                                    c.get(4).map_or(0u32, |r| r.as_str().parse().unwrap()),
                                    c.get(5).map_or(0u32, |r| r.as_str().parse().unwrap()),
                                    c.get(6).map_or(0u32, |r| r.as_str().parse().unwrap()),
                                )
                            })
                        })
                        .context("on parse by datetime regex")
                }
                _ => DateTime::parse_from_rfc3339(dt_value.as_str())
                    .or_else(|_| DateTime::parse_from_str(dt_value.as_str(), "%+"))
                    .map(Some)
                    .context("on parse rf3339"),
            };
            dt
        })
        .map_err(|e| {
            tracing::error!("caught panic: {:?}", e);
            anyhow!("error in parsing datetime: {:?}", e)
        })
        .and_then(|res| res)
    }

    async fn close(&self) -> Result<(), WebDriverError> {
        self.driver().clone().close_window().await
    }

    async fn quit(&self) -> Result<(), WebDriverError> {
        self.driver().clone().quit().await
    }

    async fn leak(&self) -> Result<(), anyhow::Error> {
        self.driver().clone().leak().map_err(|e| {
            tracing::error!("webdriver leak error: {:?}", e);
            anyhow!("webdriver leak error: {:?}", e)
        })
    }

    async fn scrape_content(
        &self,
        content_selector: &By,
        next_page_selector: Option<&By>,
    ) -> Result<Vec<String>, WebDriverError> {
        // parse contents
        let content = self.driver().find(content_selector.clone()).await?;
        if let Ok(con) = content.text().await {
            if let Some(next) = next_page_selector {
                // ページ遷移して次のコンテンツを取得する
                let r = self
                    .goto_next_content(next, content_selector, 1)
                    .await
                    .unwrap_or_else(|err| {
                        tracing::warn!("next page content not parse: {}", err);
                        vec![]
                    });
                Ok([vec![con], r].concat())
            } else {
                Ok(vec![con])
            }
        } else {
            Ok(vec![])
        }
    }

    async fn goto_next_content(
        &self,
        next: &By,
        content_selector: &By,
        page_num: i32,
    ) -> Result<Vec<String>, WebDriverError> {
        let next_ele = self.driver().find(next.clone()).await?;
        // XXX 次のページへのリンクはhrefで取得できる前提がある
        if let Some(next_url) = next_ele.attr("href").await? {
            tracing::info!("found next content: {}", &next_url);
            // ignore error for scraping (ignore not respond resource)
            if let Err(err) = self.driver().goto(next_url).await {
                tracing::warn!("loading error: {:?}", err)
            }
            let content = self.driver().find(content_selector.clone()).await?;
            let con = content.text().await?;
            // ページで空しか取れない場合は何かがおかしい可能性があり無限ループになる可能性があるため無理しないで抜ける
            if con.is_empty() {
                tracing::info!("empty page?");
                Ok(vec![])
            } else if page_num <= Self::MAX_PAGE_NUM {
                // recursive call (next page while next href exists)
                let ncon = self
                    .goto_next_content(next, content_selector, page_num + 1)
                    .await;
                if let Ok(nc) = ncon {
                    Ok([vec![con], nc].concat())
                } else {
                    Ok(vec![con])
                }
            } else {
                tracing::warn!("over max page: {}", Self::MAX_PAGE_NUM);
                Ok(vec![con])
            }
        } else {
            tracing::info!("page end");
            Ok(vec![])
        }
    }
}

impl WebScraper for WebDriverWrapper {}

#[derive(Debug, Deserialize, Serialize)]
pub struct ScrapedData {
    pub title: String,
    pub content: Vec<String>,
    pub datetime: Option<DateTime<FixedOffset>>,
    pub tags: Vec<String>,
}

// Byのwrapper
#[derive(Debug, PartialEq, Eq, EnumIter)]
pub enum SelectorType {
    ById = 1,
    ByClassName = 2,
    ByCSS = 3,
    ByXPath = 4,
    ByName = 5,
    ByTag = 6,
    ByLinkText = 7,
}

impl SelectorType {
    // n: 0初まり、nth(): 0はじまり
    pub fn new(n: i32) -> Option<SelectorType> {
        if n == 0 {
            None
        } else {
            SelectorType::iter().nth((n - 1) as usize)
        }
    }
}

impl SelectorType {
    pub fn to_by(&self, s: &str) -> By {
        match *self {
            Self::ById => By::Id(s),
            Self::ByClassName => By::ClassName(s),
            Self::ByCSS => By::Css(s),
            Self::ByXPath => By::XPath(s),
            Self::ByName => By::Name(s),
            Self::ByTag => By::Tag(s),
            Self::ByLinkText => By::LinkText(s),
        }
    }
}

use bytes::Bytes;
use http::{Request, Response};
use thirtyfour::session::http::Body;

#[async_trait::async_trait]
impl HttpClient for ReqwestClient {
    async fn send(&self, request: Request<Body<'_>>) -> WebDriverResult<Response<Bytes>> {
        let (parts, body) = request.into_parts();

        let mut req = self.client().request(parts.method, parts.uri.to_string());
        for (key, value) in parts.headers.into_iter() {
            let key = match key {
                Some(x) => x,
                None => continue,
            };
            req = req.header(key, value);
        }
        match body {
            Body::Empty => req = req.body(reqwest::Body::default()),
            Body::Json(json) => {
                req = req.json(json);
            }
        }

        let resp = req
            .send()
            .await
            .map_err(|e| WebDriverError::HttpError(format!("request error: {e:?}")))?;
        let status = resp.status();
        let mut builder = Response::builder();

        builder = builder.status(status);
        for (key, value) in resp.headers().iter() {
            builder = builder.header(key.clone(), value.clone());
        }

        let body = resp.bytes().await?;
        let body_str = String::from_utf8_lossy(&body).into_owned();
        let resp = builder
            .body(body)
            .map_err(|_| WebDriverError::UnknownResponse(status.as_u16(), body_str))?;
        Ok(resp)
    }
    async fn new(&self) -> Arc<dyn HttpClient> {
        Arc::new(self.clone())
    }
}
