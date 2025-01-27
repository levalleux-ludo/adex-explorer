#[macro_use]
extern crate seed;

use std::collections::HashMap;

use adex_domain::{AdUnit, BigNum, Channel, ChannelSpec};
use chrono::serde::ts_milliseconds;
use chrono::{DateTime, Utc};
use futures::Future;
use lazysort::*;
use num_format::{Locale, ToFormattedString};
use seed::prelude::*;
use seed::{Method, Request};
use serde::Deserialize;
use std::collections::HashSet;

const MARKET_URL: &str = "https://market.adex.network";
const VOLUME_URL: &str = "https://tom.adex.network/volume";
const IMPRESSIONS_URL: &str = "https://tom.adex.network/volume/monthly-impressions";
const ETHERSCAN_URL: &str = "https://api.etherscan.io/api";
const ETHERSCAN_API_KEY: &str = "CUSGAYGXI4G2EIYN1FKKACBUIQMN5BKR2B";
const IPFS_GATEWAY: &str = "https://ipfs.adex.network/ipfs/";
const DAI_ADDR: &str = "0x89d24A6b4CcB1B6fAA2625fE562bDD9a23260359";
const CORE_ADDR: &str = "0x333420fc6a897356e69b62417cd17ff012177d2b";
const DEFAULT_EARNER: &str = "0xb7d3f81e857692d13e9d63b232a90f4a1793189e";
const REFRESH_MS: i32 = 30000;

// Data structs specific to the market
#[derive(Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarketStatusType {
    Initializing,
    Ready,
    Active,
    Offline,
    Disconnected,
    Unhealthy,
    Withdraw,
    Expired,
    Exhausted,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MarketStatus {
    #[serde(rename = "name")]
    pub status_type: MarketStatusType,
    pub usd_estimate: f32,
    #[serde(rename = "lastApprovedBalances")]
    pub balances: HashMap<String, BigNum>,
    #[serde(with = "ts_milliseconds")]
    pub last_checked: DateTime<Utc>,
}
impl MarketStatus {
    fn balances_sum(&self) -> BigNum {
        self.balances.iter().map(|(_, v)| v).sum()
    }
}
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MarketChannel {
    pub id: String,
    pub creator: String,
    pub deposit_asset: String,
    pub deposit_amount: BigNum,
    pub status: MarketStatus,
    pub spec: ChannelSpec,
}

// Volume response from the validator
#[derive(Deserialize, Clone, Debug)]
struct VolumeResp {
    pub aggr: Vec<VolDataPoint>,
}
#[derive(Deserialize, Clone, Debug)]
struct VolDataPoint {
    pub value: BigNum,
    pub time: DateTime<Utc>,
}

// Etherscan API
#[derive(Deserialize, Clone, Debug)]
struct EtherscanBalResp {
    pub result: BigNum,
}

// Model
enum Loadable<T> {
    Loading,
    Ready(T),
}
impl<T> Default for Loadable<T> {
    fn default() -> Self {
        Loadable::Loading
    }
}
use Loadable::*;

#[derive(Clone, Copy)]
enum ChannelSort {
    Deposit,
    Status,
    Created,
}
impl Default for ChannelSort {
    fn default() -> Self {
        ChannelSort::Deposit
    }
}
// @TODO can we derive this automatically
impl From<String> for ChannelSort {
    fn from(sort_name: String) -> Self {
        match &sort_name as &str {
            "deposit" => ChannelSort::Deposit,
            "status" => ChannelSort::Status,
            "created" => ChannelSort::Created,
            _ => ChannelSort::default(),
        }
    }
}

#[derive(Default)]
struct Model {
    pub load_action: ActionLoad,
    pub sort: ChannelSort,
    // Market channels & balance: for the summaries page
    pub market_channels: Loadable<Vec<MarketChannel>>,
    pub balance: Loadable<EtherscanBalResp>,
    pub volume: Loadable<VolumeResp>,
    pub impressions: Loadable<VolumeResp>,
    // Current selected channel: for ChannelDetail
    pub channel: Loadable<Channel>,
    pub last_loaded: i64,
}

// Update
#[derive(Clone, PartialEq, Debug)]
enum ActionLoad {
    // The summary includes latest campaigns on the market,
    // and some on-chain data (e.g. DAI balance on core SC)
    Summary,
    // Channels will show the summary plus the channels
    Channels,
    // The channel detail contains a summary of what validator knows about a channel
    ChannelDetail(String),
}
impl Default for ActionLoad {
    fn default() -> Self {
        ActionLoad::Summary
    }
}
impl ActionLoad {
    fn perform_effects(&self, orders: &mut Orders<Msg>) {
        match self {
            ActionLoad::Summary | ActionLoad::Channels => {
                // Load on-chain balances
                let etherscan_uri = format!(
                    "{}?module=account&action=tokenbalance&contractAddress={}&address={}&tag=latest&apikey={}",
                    ETHERSCAN_URL,
                    DAI_ADDR,
                    CORE_ADDR,
                    ETHERSCAN_API_KEY
                );
                orders.perform_cmd(
                    Request::new(&etherscan_uri)
                        .method(Method::Get)
                        .fetch_json()
                        .map(Msg::BalanceLoaded)
                        .map_err(Msg::OnFetchErr),
                );

                // Load campaigns from the market
                // @TODO request DAI channels only
                orders.perform_cmd(
                    Request::new(&format!("{}/campaigns?all", MARKET_URL))
                        .method(Method::Get)
                        .fetch_json()
                        .map(Msg::ChannelsLoaded)
                        .map_err(Msg::OnFetchErr),
                );

                // Load volume
                orders.perform_cmd(
                    Request::new(&VOLUME_URL)
                        .method(Method::Get)
                        .fetch_json()
                        .map(Msg::VolumeLoaded)
                        .map_err(Msg::OnFetchErr),
                );
                orders.perform_cmd(
                    Request::new(&IMPRESSIONS_URL)
                        .method(Method::Get)
                        .fetch_json()
                        .map(Msg::ImpressionsLoaded)
                        .map_err(Msg::OnFetchErr),
                );
            }
            // NOTE: not used yet
            ActionLoad::ChannelDetail(id) => {
                let market_uri = format!(
                    "{}/channel/{}/events-aggregates/{}?timeframe=hour&limit=168",
                    MARKET_URL,
                    &id,
                    // @TODO get rid of this default earner thing, it's very very temporary
                    // we should get an aggr of all earners
                    DEFAULT_EARNER
                );
                // @TODO
            }
        }
    }
}

#[derive(Clone)]
enum Msg {
    Load(ActionLoad),
    Refresh,
    BalanceLoaded(EtherscanBalResp),
    ChannelsLoaded(Vec<MarketChannel>),
    VolumeLoaded(VolumeResp),
    ImpressionsLoaded(VolumeResp),
    OnFetchErr(JsValue),
    SortSelected(String),
}

fn update(msg: Msg, model: &mut Model, orders: &mut Orders<Msg>) {
    match msg {
        Msg::Load(load_action) => {
            // Do not render
            orders.skip();
            // Perform the effects
            load_action.perform_effects(orders);
            // This can be used on refresh
            model.load_action = load_action;
        }
        Msg::Refresh => {
            orders.skip();
            model.load_action.perform_effects(orders);
        }
        Msg::BalanceLoaded(resp) => model.balance = Ready(resp),
        Msg::ChannelsLoaded(channels) => {
            model.market_channels = Ready(channels);
            model.last_loaded = (js_sys::Date::now() as i64) / 1000;
        }
        Msg::VolumeLoaded(vol) => model.volume = Ready(vol),
        Msg::ImpressionsLoaded(vol) => model.impressions = Ready(vol),
        Msg::SortSelected(sort_name) => model.sort = sort_name.into(),
        // @TODO handle this
        // report via a toast
        Msg::OnFetchErr(_) => (),
    }
}

// View
fn view(model: &Model) -> El<Msg> {
    let channels = match &model.market_channels {
        Loading => return h2!["Loading..."],
        Ready(c) => c,
    };

    let total_impressions: u64 = channels
        .iter()
        .map(|x| {
            (&x.status.balances_sum() / &x.spec.min_per_impression)
                .to_u64()
                .unwrap_or(0)
        })
        .sum();

    let channels_dai = channels
        .iter()
        .filter(|MarketChannel { deposit_asset, .. }| deposit_asset == DAI_ADDR);

    let total_paid = channels_dai.clone().map(|x| x.status.balances_sum()).sum();
    let total_deposit = channels_dai
        .clone()
        .map(|MarketChannel { deposit_amount, .. }| deposit_amount)
        .sum();

    let unique_units = &channels
        .iter()
        .flat_map(|x| &x.spec.ad_units)
        .map(|x| &x.ipfs)
        .collect::<HashSet<_>>();

    let unique_publishers = channels_dai
        .clone()
        .flat_map(|x| {
            x.status
                .balances
                .keys()
                .filter(|k| **k != x.creator)
                .collect::<Vec<_>>()
        })
        .collect::<HashSet<_>>();

    div![
        // Cards
        card("Campaigns", Ready(channels.len().to_string())),
        card("Ad units", Ready(unique_units.len().to_string())),
        card("Publishers", Ready(unique_publishers.len().to_string())),
        // @TODO warn that this is an estimation; add a question mark next to it
        // to explain what an estimation means
        card(
            "Impressions",
            Ready(total_impressions.to_formatted_string(&Locale::en))
        ),
        volume_card(
            "Monthly impressions",
            match &model.impressions {
                Ready(vol) => Ready(vol.aggr
                    .iter()
                    .map(|x| &x.value)
                    .sum::<BigNum>()
                    .to_u64()
                    .unwrap_or(0)
                    .to_formatted_string(&Locale::en)),
                Loading => Loading
            },
            &model.impressions
        ),
        br![],
        card("Total campaign deposits", Ready(dai_readable(&total_deposit))),
        card("Paid out", Ready(dai_readable(&total_paid))),
        card("Locked up on-chain", match &model.balance {
            Ready(resp) => Ready(dai_readable(&resp.result)),
            Loading => Loading
        }),
        volume_card(
            "24h volume",
            match &model.volume {
                Ready(vol) => Ready(dai_readable(&vol.aggr.iter().map(|x| &x.value).sum())),
                Loading => Loading
            },
            &model.volume
        ),
        // Tables
        if model.load_action == ActionLoad::Channels {
            div![
                select![
                    attrs! {At::Value => "deposit"},
                    option![attrs! {At::Value => "deposit"}, "Sort by deposit"],
                    option![attrs! {At::Value => "status"}, "Sort by status"],
                    option![attrs! {At::Value => "created"}, "Sort by created"],
                    input_ev(Ev::Input, Msg::SortSelected)
                ],
                channel_table(
                    model.last_loaded,
                    &channels_dai
                        .clone()
                        .sorted_by(|x, y| match model.sort {
                            ChannelSort::Deposit => y.deposit_amount.cmp(&x.deposit_amount),
                            ChannelSort::Status => x.status.status_type.cmp(&y.status.status_type),
                            ChannelSort::Created => y.spec.created.cmp(&x.spec.created),
                        })
                        .collect::<Vec<_>>()
                ),
            ]
        } else {
            seed::empty()
        },
        ad_unit_stats_table(&channels_dai.clone().collect::<Vec<_>>()),
    ]
}

fn card(label: &str, value: Loadable<String>) -> El<Msg> {
    div![
        class!["card"],
        match value {
            Loading => div![class!["card-value loading"]],
            Ready(value) => div![class!["card-value"], value],
        },
        div![class!["card-label"], label],
    ]
}

fn volume_chart(vol: &VolumeResp) -> Option<El<Msg>> {
    let values = vol.aggr.iter().map(|x| &x.value);
    let min = values.clone().min()?;
    let max = values.clone().max()?;
    let range = max - min;
    let width = 250_u64;
    let height = 60_u64;
    let points = values
        .clone()
        .map(|v| {
            (&(v - min) * &height.into())
                .div_floor(&range)
                .to_u64()
                .unwrap_or(0)
        })
        .take(vol.aggr.len() - 1)
        .collect::<Vec<_>>();
    let len = points.len() as u64;
    let ratio = width as f64 / (len - 1) as f64;
    Some(svg![
        attrs! {
            At::Style => "position: absolute; right: 0px; left: 0px; bottom: 10px;";
            At::Width => format!("{}px", width);
            At::Height => format!("{}px", height);
            At::ViewBox => format!("0 0 {} {}", width, height);
        },
        polyline![attrs! {
            At::Fill => "none";
            At::Custom("stroke".into()) => "#c8dbec";
            At::Custom("stroke-width".into()) => "4";
            At::Custom("points".into()) => points
                .iter()
                .enumerate()
                .map(|(i, p)| format!("{},{}", (i as f64 * ratio).ceil(), height-p))
                .collect::<Vec<_>>()
                .join(" ")
        }],
    ])
}

fn volume_card(card_label: &str, val: Loadable<String>, vol: &Loadable<VolumeResp>) -> El<Msg> {
    let (card_value, vol) = match (&val, vol) {
        (Ready(val), Ready(vol)) => (val, vol),
        _ => return card(card_label, Loading)
    };
    match volume_chart(vol) {
        Some(chart) => div![
            class!["card chart"],
            chart,
            div![class!["card-value"], card_value],
            div![class!["card-label"], card_label],
        ],
        None => card(card_label, val),
    }
}

fn channel_table(last_loaded: i64, channels: &[&MarketChannel]) -> El<Msg> {
    let header = tr![
        td!["URL"],
        td!["USD estimate"],
        td!["Deposit"],
        td!["CPM"],
        td!["Paid"],
        td!["Paid - %"],
        //td!["Max impressions"],
        td!["Status"],
        td!["Created"],
        //td!["Last updated"],
        td!["Preview"]
    ];

    let channels = std::iter::once(header)
        .chain(channels.iter().map(|c| channel(last_loaded, c)))
        .collect::<Vec<El<Msg>>>();

    table![channels]
}

fn channel(last_loaded: i64, channel: &MarketChannel) -> El<Msg> {
    let deposit_amount = &channel.deposit_amount;
    let paid_total = channel.status.balances_sum();
    let url = format!(
        "{}/channel/{}/status",
        &channel.spec.validators.leader().url,
        channel.id
    );
    let id_prefix = channel.id.chars().take(6).collect::<String>();
    // This has a tiny issue: when you go back to the explorer after being in another window,
    // stuff will be not-recent until we get the latest status
    tr![
        class!(
            if last_loaded - channel.status.last_checked.timestamp() > 180 {
                "not-recent"
            } else {
                "recent"
            }
        ),
        td![a![
            attrs! {At::Href => url; At::Target => "_blank"},
            id_prefix
        ]],
        td![format!("${:.2}", &channel.status.usd_estimate)],
        td![dai_readable(deposit_amount)],
        td![dai_readable(
            &(&channel.spec.min_per_impression * &1000.into())
        )],
        td![dai_readable(&paid_total)],
        td![{
            let base = 100000_u64;
            let paid_units = (&paid_total * &base.into()).div_floor(deposit_amount);
            let paid_hundreds = paid_units.to_f64().unwrap_or(base as f64) / (base as f64 / 100.0);
            format!("{:.3}%", paid_hundreds)
        }],
        //td![
        //    (deposit_amount / &channel.spec.min_per_impression)
        //        .to_u64()
        //        .unwrap_or(0)
        //        .to_formatted_string(&Locale::en)
        //],
        td![format!("{:?}", &channel.status.status_type)],
        td![time_diff(last_loaded, &channel.spec.created)],
        //td![time_diff(last_loaded, &channel.status.last_checked)],
        td![class!["preview"], {
            match channel.spec.ad_units.get(0) {
                Some(unit) => a![
                    attrs! { At::Href => &unit.target_url; At::Target => "_blank" },
                    unit_preview(&unit)
                ],
                None => seed::empty(),
            }
        }]
    ]
}

fn ad_unit_stats_table(channels: &[&MarketChannel]) -> El<Msg> {
    let mut units_by_type = HashMap::<&str, Vec<&MarketChannel>>::new();
    for channel in channels.iter() {
        for unit in channel.spec.ad_units.iter() {
            units_by_type
                .entry(&unit.ad_type)
                .or_insert(vec![])
                .push(channel);
        }
    }
    let units_by_type_stats = units_by_type
        .iter()
        .map(|(ad_type, all)| {
            let total_per_impression: BigNum = all.iter().map(|x| &x.spec.min_per_impression).sum();
            // @TODO needs weighted avg
            let avg_per_impression = total_per_impression.div_floor(&(all.len() as u64).into());
            let total_vol: BigNum = all.iter().map(|x| &x.deposit_amount).sum();
            (ad_type, avg_per_impression, total_vol)
        })
        .sorted_by(|x, y| y.1.cmp(&x.1))
        .collect::<Vec<_>>();

    let header = tr![td!["Ad Size"], td!["CPM"], td!["Total volume"],];

    table![std::iter::once(header)
        .chain(
            units_by_type_stats
                .iter()
                .map(|(ad_type, avg_per_impression, total_vol)| {
                    tr![
                        td![ad_type],
                        td![dai_readable(&(avg_per_impression * &1000.into()))],
                        td![dai_readable(&total_vol)],
                    ]
                })
        )
        .collect::<Vec<El<Msg>>>()]
}

fn unit_preview(unit: &AdUnit) -> El<Msg> {
    if unit.media_mime.starts_with("video/") {
        video![
            attrs! { At::Src => to_http_url(&unit.media_url); At::AutoPlay => true; At::Loop => true; At::Muted => true }
        ]
    } else {
        img![attrs! { At::Src => to_http_url(&unit.media_url) }]
    }
}

fn to_http_url(url: &str) -> String {
    if url.starts_with("ipfs://") {
        url.replace("ipfs://", IPFS_GATEWAY)
    } else {
        url.to_owned()
    }
}

fn time_diff(now_seconds: i64, t: &DateTime<Utc>) -> String {
    let time_diff = now_seconds - t.timestamp();
    match time_diff {
        x if x < 0 => format!("just now"),
        x if x < 60 => format!("{} seconds ago", x),
        x if x < 3600 => format!("{} minutes ago", x / 60),
        x if x < 86400 => format!("{} hours ago", x / 3600),
        _ => format!("{}", t.format("%Y-%m-%d")), // %T if we want time
    }
}

fn dai_readable(bal: &BigNum) -> String {
    // 10 ** 16`
    match bal.div_floor(&10_000_000_000_000_000u64.into()).to_f64() {
        Some(hundreds) => format!("{:.2} DAI", hundreds / 100.0),
        None => ">max".to_owned(),
    }
}

// Router
fn routes(url: &seed::Url) -> Msg {
    match url.path.get(0).map(|x| x.as_ref()) {
        Some("channels") => Msg::Load(ActionLoad::Channels),
        Some("channel") => match url.path.get(1) {
            Some(id) => Msg::Load(ActionLoad::ChannelDetail(id.to_string())),
            None => Msg::Load(ActionLoad::Summary),
        },
        _ => Msg::Load(ActionLoad::Summary),
    }
}

#[wasm_bindgen]
pub fn render() {
    let state = seed::App::build(Model::default(), update, view)
        .routes(routes)
        .finish()
        .run();

    seed::set_interval(Box::new(move || state.update(Msg::Refresh)), REFRESH_MS);
}
