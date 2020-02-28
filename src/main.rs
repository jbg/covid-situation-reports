use html5ever::{
  buffer_queue::BufferQueue,
  tendril::Tendril,
  tokenizer::{StartTag, TagToken, Token, Tokenizer, TokenSink, TokenSinkResult},
};
use itertools::Itertools;
use regex::Regex;
use serde_json::json;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const BASE_URL: &str = "https://www.who.int";
const PATH: &str = "/emergencies/diseases/novel-coronavirus-2019/situation-reports/";

#[tokio::main]
async fn main() -> Result<()> {
  let latest_url = {
    let index_url = format!("{}{}", BASE_URL, PATH);
    let body = reqwest::get(&index_url)
      .await?
      .bytes()
      .await?;

    let sink = LatestSituationReportFinder::default();
    let mut tokenizer = Tokenizer::new(sink, Default::default());
    let mut queue = BufferQueue::new();
    let tendril = Tendril::try_from_byte_slice(&body)
      .map_err(|_| "Tendril::try_from_byte_slice")?;
    queue.push_back(tendril);
    let _ = tokenizer.feed(&mut queue);
    tokenizer.end();
    tokenizer.sink.url
  };

  if let Some(url) = latest_url {
    let body = reqwest::get(&url)
      .await?
      .bytes()
      .await?;
    let document = lopdf::Document::load_mem(&body)?;
    let page_numbers: Vec<u32> = document.get_pages().keys().copied().collect();
    let cases_re = Regex::new(r#"(?x)
      ^\s*\d+(\s+\(\s*\d+\s*\))?\s*$
    "#)?;
    let text = document.extract_text(&page_numbers)?;
    let mut all_regions_iter = text
        .lines()
        .filter_map(|line| match line.trim() {
          "" => None,
          line => Some(line.to_string()),
        })
        .skip_while(|line| line != "Hubei")
        .take_while(|line| line != "Case classifications are")
        .coalesce(|prev, cur| if cur.starts_with("(") && cases_re.is_match(&prev) && !prev.contains('(') {
          Ok(format!("{} {}", prev, cur))
        }
        else {
          Err((prev, cur))
        })
        .batching(|it| {
          let mut preamble: Vec<_> = it
            .take_while_ref(|line| !cases_re.is_match(line))
            .filter(|line| !(
              line.contains("Region")
              || line.contains(" - ")
              || line.contains("Unimplemented?")
            ))
            .collect();
          let mut preamble = if preamble.iter().any(|el| el == "Country/Territory/Area") {
            preamble.pop().unwrap()
          }
          else {
            preamble.join(" ")
          };
          if preamble.starts_with(")") {
            preamble = preamble.chars().skip(1).collect();
          }
          if preamble.ends_with("§") {
            let count = preamble.chars().count();
            preamble = preamble.chars().take(count - 1).collect();
          }
          preamble = preamble
            .trim()
            .replace("Total", "China")
            .replace("Uni ted", "United")
            .replace("Finlan d", "Finland")
            .replace("Jian gsu", "Jiangsu")
            .replace("South - ", "");
          let counts: Vec<_> = it
            .take_while_ref(|line| cases_re.is_match(line))
            .map(|count| {
              if count.contains("(") {
                count.split(|c| c == '(' || c == ')')
                  .take(2)
                  .map(|c|
                    c.trim()
                      .parse::<u32>()
                      .map_err(|_| format!("failed to parse: {:?}", c))
                      .unwrap()
                  )
                  .collect()
              }
              else {
                vec![
                  count.parse::<u32>()
                    .map_err(|_| format!("failed to parse: {:?}", count))
                    .unwrap()
                ]
              }
            })
            .collect();
          if preamble.is_empty() || counts.is_empty() {
            None
          }
          else {
            Some((preamble, counts))
          }
        })
        .filter(|(preamble, counts)|
          counts.len() >= 6
          && preamble != "Subtotal for all regions"
          && preamble != "Grand total"
        );
    let china_regions: Vec<_> = all_regions_iter
      .take_while_ref(|(region, _counts)| region != "China")
      .map(|(region_name, counts)| {
        json!({
          "name": region_name,
          "population": counts[0][0],
          "today_confirmed_cases": counts[1][0],
          "today_suspected_cases": counts[2][0],
          "today_deaths": counts[3][0],
          "total_confirmed_cases": counts[4][0],
          "total_deaths": counts[5][0]
        })
      })
      .collect();
    let countries: Vec<_> = all_regions_iter
      .map(|(country_name, counts)| {
        if country_name == "China" {
          json!({
            "name": "China",
            "today_confirmed_cases": counts[1][0],
            "today_suspected_cases": counts[2][0],
            "total_confirmed_cases": counts[4][0],
            "today_likely_place_of_exposure_china": null,
            "total_likely_place_of_exposure_china": null,
            "today_likely_place_of_exposure_in_country": null,
            "total_likely_place_of_exposure_in_country": null,
            "today_likely_place_of_exposure_other": null,
            "total_likely_place_of_exposure_other": null,
            "today_likely_place_of_exposure_unknown": null,
            "total_likely_place_of_exposure_unknown": null,
            "today_deaths": counts[3][0],
            "total_deaths": counts[5][0],
            "regions": &china_regions
          })
        }
        else {
          json!({
            "name": country_name,
            "today_confirmed_cases": counts[0][1],
            "today_suspected_cases": null,
            "total_confirmed_cases": counts[0][0],
            "today_likely_place_of_exposure_china": counts[1][1],
            "total_likely_place_of_exposure_china": counts[1][0],
            "today_likely_place_of_exposure_in_country": counts[3][1],
            "total_likely_place_of_exposure_in_country": counts[3][0],
            "today_likely_place_of_exposure_other": counts[2][1],
            "total_likely_place_of_exposure_other": counts[2][0],
            "today_likely_place_of_exposure_unknown": counts[4][1],
            "total_likely_place_of_exposure_unknown": counts[4][0],
            "today_deaths": counts[5][1],
            "total_deaths": counts[5][0],
            "regions": null
          })
        }
      })
      .collect();
    println!("{}", serde_json::to_string_pretty(&countries)?);
  }
  else {
    panic!("URL for PDF not found in HTML document");
  }

  Ok(())
}

#[derive(Default)]
struct LatestSituationReportFinder {
  url: Option<String>,
}

impl TokenSink for LatestSituationReportFinder {
  type Handle = ();

  fn process_token(&mut self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
    match token {
      TagToken(tag)
        if tag.kind == StartTag
        && tag.name.to_string() == "a"
      => {
        if let Some(href) = tag.attrs.iter().find(|a| &a.name.local == "href") {
          if href.value.starts_with("/docs/default-source/coronaviruse/situation-reports/") {
            self.url = Some(format!("{}{}", BASE_URL, href.value));
            return TokenSinkResult::Plaintext;
          }
        }
      },
      _ => {},
    }
    TokenSinkResult::Continue
  }
}
