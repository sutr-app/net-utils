use crate::net::reqwest::ReqwestClient;
use anyhow::{Context, Result};
use async_trait::async_trait;
use atom_syndication::{Entry, Feed, Link, Text};
use rss::{Category, Channel, Enclosure, Guid, Item};
use std::time::Duration;

pub trait UseReqwest {
    fn reqwest_client(&self) -> &ReqwestClient;
}

#[async_trait]
pub trait RssClient: UseReqwest {
    // async fn create_list(&self, url: &str) -> Result<Channel> {
    //     let channel = self.request_feed(url).await?;
    //     println!("rss channel info: {:?}", channel);
    //     for item in channel.items() {
    //         println!("channel item: {:?}", item)
    //     }
    //     Ok(channel)
    // }

    async fn request_feed(&self, url: &str) -> Result<Channel> {
        let content = self
            .reqwest_client()
            .client()
            .get(url)
            .send()
            .await
            .context(format!("In reqwest url: {url}"))?
            .bytes()
            .await?;
        parse_feed_content(&content).context(format!("In read rss url: {url}"))
    }
}

fn parse_feed_content(content: &[u8]) -> Result<Channel> {
    match Channel::read_from(content) {
        Ok(channel) => Ok(channel),
        Err(rss_error) => Feed::read_from(content)
            .map(atom_feed_to_rss_channel)
            .with_context(|| format!("failed to parse feed as RSS or Atom: {rss_error}")),
    }
}

fn atom_feed_to_rss_channel(feed: Feed) -> Channel {
    let mut channel = Channel::default();
    channel.set_title(atom_text_value(feed.title()));
    channel.set_link(atom_link_href(feed.links()).unwrap_or_else(|| feed.id().to_owned()));
    channel.set_description(
        feed.subtitle()
            .or_else(|| feed.rights())
            .map_or_else(String::new, atom_text_value),
    );
    channel.set_language(feed.lang.clone());
    channel.set_copyright(feed.rights().map(atom_text_value));
    channel.set_last_build_date(feed.updated().to_rfc2822());
    channel.set_categories(
        feed.categories()
            .iter()
            .map(atom_category_to_rss_category)
            .collect::<Vec<_>>(),
    );
    channel.set_generator(feed.generator.map(|generator| generator.value));
    channel.set_items(
        feed.entries
            .into_iter()
            .map(atom_entry_to_rss_item)
            .collect::<Vec<_>>(),
    );
    channel
}

fn atom_entry_to_rss_item(entry: Entry) -> Item {
    let link = atom_link_href(entry.links());
    let mut item = Item::default();
    item.set_title(atom_text_value(entry.title()));
    item.set_link(link.clone());
    item.set_description(atom_entry_description(&entry));
    item.set_author(
        entry
            .authors()
            .first()
            .map(|author| author.name().to_owned()),
    );
    item.set_categories(
        entry
            .categories()
            .iter()
            .map(atom_category_to_rss_category)
            .collect::<Vec<_>>(),
    );
    item.set_enclosure(atom_entry_enclosure(&entry));
    item.set_guid(Guid {
        value: if entry.id().is_empty() {
            link.unwrap_or_default()
        } else {
            entry.id().to_owned()
        },
        permalink: false,
    });
    item.set_pub_date(
        entry
            .published()
            .unwrap_or_else(|| entry.updated())
            .to_rfc2822(),
    );
    item.set_content(entry.content().and_then(|content| {
        content
            .value()
            .map(str::to_owned)
            .or_else(|| content.src().map(str::to_owned))
    }));
    item
}

fn atom_entry_description(entry: &Entry) -> Option<String> {
    entry.summary().map(atom_text_value).or_else(|| {
        entry
            .content()
            .and_then(|content| content.value().map(str::to_owned))
    })
}

fn atom_entry_enclosure(entry: &Entry) -> Option<Enclosure> {
    entry
        .links()
        .iter()
        .find(|link| link.rel() == "enclosure")
        .map(|link| Enclosure {
            url: link.href().to_owned(),
            length: link.length().unwrap_or_default().to_owned(),
            mime_type: link.mime_type().unwrap_or_default().to_owned(),
        })
}

fn atom_category_to_rss_category(category: &atom_syndication::Category) -> Category {
    Category {
        name: category
            .label()
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| category.term())
            .to_owned(),
        domain: category.scheme().map(str::to_owned),
    }
}

fn atom_link_href(links: &[Link]) -> Option<String> {
    links
        .iter()
        .find(|link| link.rel() == "alternate" && !link.href().is_empty())
        .or_else(|| links.iter().find(|link| !link.href().is_empty()))
        .map(|link| link.href().to_owned())
}

fn atom_text_value(text: &Text) -> String {
    text.as_str().to_owned()
}

pub trait UseRssClient {
    type CLIENT: RssClient + Send + Sync;
    fn rss_client(&self) -> &Self::CLIENT;
}

pub struct RssClientImpl {
    reqwest_client: ReqwestClient,
}

impl RssClientImpl {
    pub fn new(
        user_agent: impl Into<String>,
        timeout: Duration,
        connect_timeout: Duration,
    ) -> Self {
        // TODO あとでちゃんと作る
        let client = ReqwestClient::new(
            Some(user_agent.into().as_str()),
            Some(timeout),
            Some(connect_timeout),
            Some(2),
        )
        .unwrap();
        Self {
            reqwest_client: client,
        }
    }
}

impl UseReqwest for RssClientImpl {
    fn reqwest_client(&self) -> &ReqwestClient {
        &self.reqwest_client
    }
}

impl RssClient for RssClientImpl {}

#[cfg(test)]
mod tests {
    use super::parse_feed_content;

    #[test]
    fn parse_feed_content_reads_rss() {
        let channel = parse_feed_content(
            br#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>RSS Feed</title>
    <link>https://example.com/</link>
    <description>RSS description</description>
    <item>
      <title>RSS Item</title>
      <link>https://example.com/rss-item</link>
      <description>RSS item description</description>
      <pubDate>Wed, 24 Jun 2026 20:00:25 +0000</pubDate>
    </item>
  </channel>
</rss>"#,
        )
        .expect("RSS should parse");

        assert_eq!(channel.title(), "RSS Feed");
        assert_eq!(channel.link(), "https://example.com/");
        assert_eq!(channel.items().len(), 1);
        assert_eq!(channel.items()[0].title(), Some("RSS Item"));
    }

    #[test]
    fn parse_feed_content_reads_atom_as_rss_channel() {
        let channel = parse_feed_content(
            br#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Feed</title>
  <subtitle>Atom subtitle</subtitle>
  <link href="https://example.com/"/>
  <updated>2026-06-24T20:00:25Z</updated>
  <id>https://example.com/feed.xml</id>
  <entry>
    <title>Atom Entry</title>
    <link href="https://example.com/atom-entry"/>
    <id>https://example.com/atom-entry</id>
    <updated>2026-06-24T20:00:25Z</updated>
    <summary>Atom entry summary</summary>
  </entry>
</feed>"#,
        )
        .expect("Atom should parse");

        assert_eq!(channel.title(), "Atom Feed");
        assert_eq!(channel.link(), "https://example.com/");
        assert_eq!(channel.description(), "Atom subtitle");
        assert_eq!(
            channel.last_build_date(),
            Some("Wed, 24 Jun 2026 20:00:25 +0000")
        );
        assert_eq!(channel.items().len(), 1);
        assert_eq!(channel.items()[0].title(), Some("Atom Entry"));
        assert_eq!(
            channel.items()[0].link(),
            Some("https://example.com/atom-entry")
        );
        assert_eq!(channel.items()[0].description(), Some("Atom entry summary"));
        assert_eq!(
            channel.items()[0].guid().map(|guid| guid.value()),
            Some("https://example.com/atom-entry")
        );
        assert_eq!(
            channel.items()[0].pub_date(),
            Some("Wed, 24 Jun 2026 20:00:25 +0000")
        );
    }

    #[test]
    fn parse_feed_content_rejects_unknown_xml() {
        let err = parse_feed_content(br#"<?xml version="1.0"?><root />"#)
            .expect_err("Unknown XML should be rejected");

        assert!(
            err.to_string()
                .contains("failed to parse feed as RSS or Atom")
        );
    }
}
