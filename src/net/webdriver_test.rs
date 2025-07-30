
#[tokio::test]
// #[ignore]
async fn scraping_test() {
    use std::sync::Arc;

    let config = WebDriverConfig {
     url: Some("http://localhost:9515".to_string()),
     user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36 Edg/108.0.1462.54".to_string()),
     page_load_timeout_sec: Some(Duration::from_secs(30)),
     script_timeout_sec: Some(Duration::from_secs(30)),
     pool_max_size: 1,
     pool_timeout_wait_sec: Some(Duration::from_secs(30)),
     pool_timeout_create_sec: Some(Duration::from_secs(30)),
     pool_timeout_recycle_sec:Some(Duration::from_secs(30)),
    };
    let web_driver_pool = Arc::new(WebDriverPool::new(config).await);
    let driver = web_driver_pool
        .get()
        .await
        .map_err(|e| {
            println!("error: {:?}", e);
            e
        })
        .unwrap();
    let data = driver
        .scraping(
            "https://note.com/vaaaaanquish/n/n6d5196f13988",
            By::Css("h1.o-noteContentHeader__title"),
            By::Css("div[data-name=\"body\"]"),
            Some(By::Css(".o-noteContentHeader__date time")),
            &Some("datetime".to_string()),
            &None,
            None,
            Some(By::ClassName("m-tagList__item")),
        )
        .await
        .unwrap();
    println!("data: {:?}", &data);
    assert!(!data.content.is_empty());
    assert_eq!(data.tags.len(), 5);
}

// yahooニュースは一定期間で消えることがおおそうなので一旦コメントアウト
// TODO トップからあるものを拾えるようにする
// #[tokio::test]
// async fn scraping_pages_test() -> Result<(), WebDriverError> {
//     use std::sync::Arc;
//     let config = WebDriverConfig {
//         url: Some("http://selenium-hub.selenium.svc.cluster.local:4444".to_string()),
//         user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36 Edg/108.0.1462.54".to_string()),
//         page_load_timeout_sec: Some(Duration::from_secs(30)),
//         script_timeout_sec: Some(Duration::from_secs(30)),
//         pool_max_size: 1,
//         pool_timeout_wait_sec: Some(Duration::from_secs(30)),
//         pool_timeout_create_sec: Some(Duration::from_secs(30)),
//         pool_timeout_recycle_sec:Some(Duration::from_secs(30)),
//     };
//     let web_driver_pool = Arc::new(WebDriverPool::new(config).await);
//     let driver = web_driver_pool.get().await.unwrap();
//     let data = driver
//         .scraping(
//             "https://news.yahoo.co.jp/articles/85663708926d02e4ea66ac43d016ed3d5e077f59",
//             By::Css("article header h1"),
//             By::Css("div.article_body, section.articleBody"),
//             Some(By::Css("header div div div time")),
//             &None,
//             &Some(r#"(?:(\d+)/)?(\d+)/(\d+)\(.*\)[^\d]*(\d+):(\d+)(?::(\d+))?"#.to_string()), // "9/4(日) 6:03",
//             Some(By::LinkText("次へ")),
//             None,
//         )
//         .await?;
//     println!("data: {:?}", &data);
//     assert!(data.datetime.is_some());
//     assert!(!data.content.is_empty());
//     assert_eq!(data.content.len(), 8);
//     driver.quit().await
// }

#[tokio::test]
#[ignore]
async fn scraping_hatena_test() {
    use std::sync::Arc;
    let config = WebDriverConfig {
        url: Some("http://localhost:9515".to_string()),
        user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36 Edg/108.0.1462.54".to_string()),
        page_load_timeout_sec: Some(Duration::from_secs(30)),
        script_timeout_sec: Some(Duration::from_secs(30)),
        pool_max_size: 1,
        pool_timeout_wait_sec: Some(Duration::from_secs(30)),
        pool_timeout_create_sec: Some(Duration::from_secs(30)),
        pool_timeout_recycle_sec:Some(Duration::from_secs(30)),
    };
    let web_driver_pool = Arc::new(WebDriverPool::new(config).await);
    let driver = web_driver_pool.get().await.unwrap();
    let data = driver
        .scraping(
            "https://tomohiro358.hatenablog.com/entry/2022/09/11/211317",
            By::Css("h1.entry-title"),
            By::Css("div.entry-content"),
            Some(By::Css("time[data-relative]")),
            &Some("datetime".to_string()),
            &None,
            None,
            Some(By::Css("span.entry-tag")),
        )
        .await
        .unwrap();
    println!("data: {:?}", &data);
    assert!(data.datetime.is_some());
    assert!(!data.content.is_empty());
    assert_eq!(data.content.len(), 1);
    assert_eq!(data.tags.len(), 5);
}

#[tokio::test]
#[ignore]
async fn scraping_anond_test() {
    use std::sync::Arc;
    let config = WebDriverConfig {
        url: Some("http://localhost:9515".to_string()),
        user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36 Edg/108.0.1462.54".to_string()),
        page_load_timeout_sec: Some(Duration::from_secs(30)),
        script_timeout_sec: Some(Duration::from_secs(30)),
        pool_max_size: 1,
        pool_timeout_wait_sec: Some(Duration::from_secs(30)),
        pool_timeout_create_sec: Some(Duration::from_secs(30)),
        pool_timeout_recycle_sec:Some(Duration::from_secs(30)),
    };
    let web_driver_pool = Arc::new(WebDriverPool::new(config).await);
    let driver = web_driver_pool.get().await.unwrap();
    let data = driver
        .scraping(
            "https://anond.hatelabo.jp/20230123185156",
            By::Css(".section h3"),
            By::Css("div.section"),
            Some(By::Css(".section h3 a[href]")),
            &Some("href".to_string()),
            &Some(r#"/(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})"#.to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    println!("data: {:?}", &data);
    assert!(data.datetime.is_some());
    assert!(!data.content.is_empty());
    assert_eq!(data.content.len(), 1);
}

#[tokio::test]
#[ignore]
async fn scraping_impress_test() {
    use std::sync::Arc;
    let config = WebDriverConfig {
        url: Some("http://localhost:9515".to_string()),
        user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36 Edg/108.0.1462.54".to_string()),
        page_load_timeout_sec: Some(Duration::from_secs(30)),
        script_timeout_sec: Some(Duration::from_secs(30)),
        pool_max_size: 1,
        pool_timeout_wait_sec: Some(Duration::from_secs(30)),
        pool_timeout_create_sec: Some(Duration::from_secs(30)),
        pool_timeout_recycle_sec:Some(Duration::from_secs(30)),
    };
    let web_driver_pool = Arc::new(WebDriverPool::new(config).await);
    let driver = web_driver_pool.get().await.unwrap();
    let data = driver
        .scraping(
            "https://k-tai.watch.impress.co.jp/docs/news/1516774.html",
            By::Css("div.title-header div h1"),
            By::Css("div.main-contents"),
            Some(By::Css("p.publish-date")),
            &None,
            &Some(r#"(\d+)年(\d+)月(\d+)日\s+(\d+):(\d+)"#.to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    println!("data: {:?}", &data);
    assert!(data.datetime.is_some());
    assert!(!data.content.is_empty());
    assert_eq!(data.content.len(), 1);
}

#[tokio::test]
#[ignore]
async fn scraping_techno_edge_test() {
    use std::sync::Arc;
    let config = WebDriverConfig {
        url: Some("http://localhost:9515".to_string()),
        user_agent: Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/108.0.0.0 Safari/537.36 Edg/108.0.1462.54".to_string()),
        page_load_timeout_sec: Some(Duration::from_secs(30)),
        script_timeout_sec: Some(Duration::from_secs(30)),
        pool_max_size: 1,
        pool_timeout_wait_sec: Some(Duration::from_secs(30)),
        pool_timeout_create_sec: Some(Duration::from_secs(30)),
        pool_timeout_recycle_sec:Some(Duration::from_secs(30)),
    };
    let web_driver_pool = Arc::new(WebDriverPool::new(config).await);
    let driver = web_driver_pool.get().await.unwrap();
    let data = driver
        .scraping(
            "https://www.techno-edge.net/article/2023/11/13/2252.html",
            By::Css("header.arti-header h1.head"),
            By::Css("article.arti-body"),
            Some(By::Css("time.pubdate")),
            &Some("datetime".to_string()),
            &None,
            None,
            None,
        )
        .await
        .unwrap();
    println!("data: {:?}", &data);
    assert!(data.datetime.is_some());
    assert!(!data.content.is_empty());
    assert_eq!(data.content.len(), 1);
}
