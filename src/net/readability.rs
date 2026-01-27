use anyhow::{Result, anyhow};
use command_utils::util::encoding;
use readability::extractor::Product;
use reqwest::{self, StatusCode};
use robotstxt::DefaultMatcher;
use std::{borrow::BorrowMut, io::Cursor, time::Duration};
use url::Url;

use memory_utils::cache::stretto::{MemoryCacheConfig, MemoryCacheImpl, UseMemoryCache};

use super::{
    reqwest::ReqwestClient,
    webdriver::{UseWebDriver, WebDriverWrapper},
};

static MEMORY_CACHE: once_cell::sync::Lazy<MemoryCacheImpl<String, Option<String>>> =
    once_cell::sync::Lazy::new(|| {
        let conf = MemoryCacheConfig::default();
        MemoryCacheImpl::<String, Option<String>>::new(
            &conf,
            Some(Duration::from_secs(60 * 60 * 24)),
        )
    });

fn robots_txt_url(url_str: &str) -> Result<Url> {
    let mut url = Url::parse(url_str)?;
    url.set_path("/robots.txt");
    url.set_fragment(None);
    url.set_query(None);
    Ok(url)
}

pub async fn get_robots_txt(
    url_str: &str,
    user_agent: Option<&str>,
    timeout: Option<Duration>,
) -> Result<Option<String>> {
    let robots_url = robots_txt_url(url_str)?;
    MEMORY_CACHE
        .with_cache_locked(
            &robots_url.as_str().to_string(),
            Some(&Duration::from_secs(60 * 60 * 24)),
            || async {
                // cache test to ses
                // println!("========= request to robots.txt: {}", robots_url);
                let client = reqwest::Client::builder()
                    .timeout(timeout.unwrap_or_else(|| Duration::new(30, 0)))
                    .connect_timeout(timeout.unwrap_or_else(|| Duration::new(30, 0)));
                let client = if let Some(ua) = user_agent {
                    client.user_agent(ua).build()?
                } else {
                    client.build()?
                };

                let res = client.get(robots_url.as_str()).send().await?;
                if res.status().is_success() {
                    let txt = res
                        .text()
                        .await
                        .map_err(|e| anyhow!("content error: {:?}", e))?;
                    Ok(Some(txt))
                } else if res.status() == StatusCode::NOT_FOUND {
                    Ok(None)
                } else {
                    Err(anyhow!("robot_txt request not success: {:?}", res))
                }
                // let client = ReqwestClient::new(user_agent, Some(Duration::new(30, 0)), Some(2))?;
                // let res = client.client().get(robots_url.as_str()).send().await?;
                // if res.status().is_success() {
                //     let txt = res
                //         .text()
                //         .await
                //         .map_err(|e| anyhow!("content error: {:?}", e))?;
                //     Ok(Some(txt))
                // } else if res.status() == StatusCode::NOT_FOUND {
                //     Ok(None)
                // } else {
                //     Err(anyhow!("robot_txt request not success: {:?}", res))
                // }
            },
        )
        .await
}

// TODO make struct, with caching
fn available_url_by_robots_txt(robots_txt: &str, url: &str, user_agent: &str) -> bool {
    let mut matcher = DefaultMatcher::default();
    matcher.one_agent_allowed_by_robots(robots_txt, user_agent, url)
}

pub async fn readable_by_robots_txt(
    url_str: &str,
    user_agent: Option<&str>,
) -> Result<Option<bool>> {
    let url = Url::parse(url_str)?;
    let bot_ua = "curl/8.7.1"; // for fetching robots.txt only
    let robots_txt = get_robots_txt(url_str, Some(bot_ua), None).await?;
    Ok(robots_txt.map(|t| {
        available_url_by_robots_txt(t.as_str(), url.as_str(), user_agent.unwrap_or("robotstxt"))
    }))
}

pub async fn request_to_utf8(
    url: &str,
    user_agent: Option<&str>,
    check_robotstxt: bool,
) -> Result<Product> {
    if check_robotstxt {
        let robotstxt = readable_by_robots_txt(url, user_agent).await?;
        if !robotstxt.unwrap_or(true) {
            Err(anyhow!("denied by robots.txt"))?
        }
    }
    let client = ReqwestClient::new(user_agent, Some(Duration::new(30, 0)), None, Some(2))?;
    let res = client.client().get(url).send().await?;
    if res.status().is_success() {
        let url = Url::parse(url)?;
        // add to encode to utf8 (all in buffer)
        let mut res = encoding::encode_to_utf8_raw(res.bytes().await?.borrow_mut())?;
        let mut c = unsafe { Cursor::new(res.as_bytes_mut()) };
        readability::extractor::extract(&mut c, &url)
            .map_err(|e| anyhow!("readable extract error: {:?}", e))
    } else {
        Err(anyhow!("request not success: {:?}", res))
    }
}

pub async fn request_by_webdriver(
    webdriver: &WebDriverWrapper,
    url: &str,
    user_agent: Option<&str>,
    check_robotstxt: bool,
) -> Result<Product> {
    if check_robotstxt {
        let robotstxt = readable_by_robots_txt(url, user_agent).await?;
        if !robotstxt.unwrap_or(true) {
            Err(anyhow!("denied by robots.txt"))?
        }
    }
    if let Err(err) = webdriver.driver().goto(url).await {
        tracing::warn!("loading error: {:?}", err)
    }
    let status = webdriver.driver().status().await?;
    // self.driver().screenshot(path::Path::new("./browser_screen.png")).await?;
    let source = webdriver.driver().source().await?;
    tracing::trace!("page source:{:?}", source);
    if status.ready {
        let url = Url::parse(url)?;
        // add to encode to utf8 (all in buffer)
        let mut res = encoding::encode_to_utf8_raw(source.as_bytes())?;
        let mut c = unsafe { Cursor::new(res.as_bytes_mut()) };
        readability::extractor::extract(&mut c, &url)
            .map_err(|e| anyhow!("readable extract error: {:?}", e))
    } else {
        Err(anyhow!("request not success: {:?}", status.message))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[ignore]
    #[tokio::test]
    async fn test_request_euc() {
        // euc-jp
        let url = "https://www.4gamer.net/games/535/G053589/20240105025/";
        let result = request_to_utf8(url, Some("Request/1.0"), true)
            .await
            .unwrap();
        println!("==== title: {}", &result.title);
        println!("=== text: {}", &result.text);
        println!("=== content: {}", &result.content);
        println!("=== url: {}", &url);
        assert!(!result.text.is_empty());
    }

    #[test]
    fn test_robots_txt_url() {
        assert_eq!(
            robots_txt_url("https://www.example.com").unwrap().as_str(),
            "https://www.example.com/robots.txt"
        );
        assert_eq!(
            robots_txt_url("https://www.example.com/search?hoge=fuga")
                .unwrap()
                .as_str(),
            "https://www.example.com/robots.txt"
        );
        assert_eq!(
            robots_txt_url("https://www.example.com/search/?hoge=fuga#id")
                .unwrap()
                .as_str(),
            "https://www.example.com/robots.txt"
        );
    }
    // #[ignore = "need to setup test server"]
    // #[tokio::test]
    // async fn test_get_robots_txt() {
    //     let url = "https://www.yahoo.co.jp";
    //     let txt = get_robots_txt(url, Some("Request/1.0"), Some(Duration::new(30, 0)))
    //         .await
    //         .unwrap();
    //     println!("robots.txt: {:?}", txt);
    //     assert!(txt.is_some());
    //     let txt = get_robots_txt(url, Some("Request/1.0"), Some(Duration::new(30, 0)))
    //         .await
    //         .unwrap();
    //     println!("robots.txt2: {:?}", txt);
    //     assert!(txt.is_some());
    //     let txt = get_robots_txt(url, Some("Request/1.0"), Some(Duration::new(30, 0)))
    //         .await
    //         .unwrap();
    //     println!("robots.txt3: {:?}", txt);
    //     assert!(txt.is_some());
    // }
}
