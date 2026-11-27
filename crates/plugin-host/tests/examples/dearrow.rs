use rekindle_plugin_host::runtime::RestRequest;
use rekindle_plugin_host::testing::Harness;
use serde_json::{Value, json};

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/dearrow");
const BRANDING: &str = "https://sponsor.ajay.app/api/branding?videoID=dQw4w9WgXcQ";

fn history() -> RestRequest {
    RestRequest {
        id: 1,
        method: "GET".into(),
        path: "/channels/7/messages?limit=50".into(),
        body: None,
    }
}

fn messages() -> String {
    json!([{
        "id": "1",
        "content": "https://youtu.be/dQw4w9WgXcQ",
        "embeds": [{
            "type": "video",
            "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "title": "YOU WON'T BELIEVE THIS",
            "thumbnail": {"url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/hq.jpg", "proxy_url": "https://images-ext-1.discordapp.net/x"}
        }]
    }])
    .to_string()
}

fn branding() -> String {
    json!({
        "titles": [{"title": "A song about commitment", "original": false, "votes": 3, "locked": false}],
        "thumbnails": [{"timestamp": 42.5, "original": false, "votes": 1, "locked": false}]
    })
    .to_string()
}

#[test]
fn a_youtube_embed_gets_dearrows_title_and_thumbnail() {
    let mut h = Harness::builder(PLUGIN)
        .http_get(BRANDING, 200, branding())
        .start()
        .unwrap();
    let read = h
        .runtime()
        .filter_response(&history(), 200, Some(messages()))
        .unwrap();
    let read: Value = serde_json::from_str(&read).unwrap();
    let embed = &read[0]["embeds"][0];
    assert_eq!(embed["title"], "A song about commitment");
    let thumb = "https://dearrow-thumb.ajay.app/api/v1/getThumbnail?videoID=dQw4w9WgXcQ&time=42.5";
    assert_eq!(embed["thumbnail"]["url"], thumb);
    assert_eq!(embed["thumbnail"]["proxy_url"], thumb);
    h.assert_active();
}

#[test]
fn each_video_is_looked_up_once() {
    let mut h = Harness::builder(PLUGIN)
        .http_get(BRANDING, 200, branding())
        .start()
        .unwrap();
    for _ in 0..3 {
        h.runtime()
            .filter_response(&history(), 200, Some(messages()));
    }
    assert_eq!(h.requests().len(), 1);
}

#[test]
fn a_video_dearrow_doesnt_know_stays_as_it_is() {
    let mut h = Harness::builder(PLUGIN)
        .http_get(BRANDING, 404, "")
        .start()
        .unwrap();
    let read = h
        .runtime()
        .filter_response(&history(), 200, Some(messages()));
    assert_eq!(read, Some(messages()));
}
