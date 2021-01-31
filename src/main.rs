// Goals
// 1. Make a request to the server for a given string, and return a count back for visits
//
// TODO:
// Persistence
// 1=View, N=Views
use argh::FromArgs;
use http::{Response, StatusCode};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fmt, net,
    sync::{Arc, Mutex},
};
use tokio::{
    fs::File,
    io::{self, AsyncReadExt},
};
use warp::Filter;

#[derive(Debug)]
struct ViewCountSVG {
    before_color_part: String,
    after_color_before_text_part: String,
    after_text_part: String,
}

#[derive(Debug)]
enum Error {
    /// Used when parsing a ViewCountSVG fails
    MissingPart,

    IO(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MissingPart => write!(f, "MissingPart"),
            Error::IO(io_err) => write!(f, "IO: {}", &io_err),
        }
    }
}
impl std::error::Error for Error {}

impl ViewCountSVG {
    async fn from_file<P>(path: P, pattern: &str) -> Result<ViewCountSVG, Error>
    where
        P: AsRef<std::path::Path>,
    {
        let mut file = File::open(path).await.map_err(Error::IO)?;
        let mut buffer = String::new();

        file.read_to_string(&mut buffer).await.map_err(Error::IO)?;

        let mut read_iterator = buffer.split(pattern);

        Ok(ViewCountSVG {
            before_color_part: read_iterator
                .next()
                .ok_or_else(|| Error::MissingPart)?
                .to_string(),
            after_color_before_text_part: read_iterator
                .next()
                .ok_or_else(|| Error::MissingPart)?
                .to_string(),
            after_text_part: read_iterator
                .next()
                .ok_or_else(|| Error::MissingPart)?
                .to_string(),
        })
    }

    fn template(&self, css_color_str: &str, view_count: &str) -> String {
        let mut buf = String::with_capacity(
            self.before_color_part.len()
                + self.after_color_before_text_part.len()
                + self.after_text_part.len(),
        );

        buf.push_str(&*self.before_color_part);
        buf.push_str(css_color_str);
        buf.push_str(&*self.after_color_before_text_part);
        buf.push_str(view_count);
        buf.push_str(&*self.after_text_part);

        buf
    }
}

#[derive(Debug)]
struct ColorScale {
    colors: Vec<String>,
    max_views: u64,
}

impl ColorScale {
    async fn from_file<P>(path: P, max_views: u64) -> io::Result<ColorScale>
    where
        P: AsRef<std::path::Path>,
    {
        let mut file = File::open(path).await?;
        let mut buffer = String::new();

        file.read_to_string(&mut buffer).await?;

        Ok(ColorScale {
            max_views,
            colors: buffer
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
        })
    }

    /// linear interpolation
    fn hex_color_for_view_count(&self, view_count: u64) -> &str {
        let views = std::cmp::min(view_count, self.max_views) as f64;
        let (x1, y1) = (0f64, 0f64);
        let (x2, y2) = (self.max_views as f64, (self.colors.len() - 1) as f64);
        let ratio = (y2 - y1) / (x2 - x1);
        let interp = y1 + (views - x1) * ratio;
        // we cap the value with the cmp::min above, so we can use unchecked access
        &self.colors[interp.floor() as usize]
    }

    fn random_hex_color(&self) -> &str {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        &self.colors[rng.gen_range(0..self.colors.len() - 1)]
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum FillMode {
    Random,
    MIlestone,
}

#[derive(Deserialize)]
struct QueryParameters {
    fill_mode: Option<FillMode>,
}

// TODO(haze): custom paths for colors and template
#[derive(FromArgs)]
/// profile_view_counter server
struct Options {
    /// host on all interfaces or not
    #[argh(switch, short = 'i')]
    host_on_all_interfaces: bool,

    /// what port to host on
    #[argh(option, short = 'p')]
    port: Option<u16>,

    /// max number of views to count
    #[argh(option)]
    max_views: Option<u64>,
}

impl Options {
    fn max_views(&self) -> u64 {
        self.max_views.unwrap_or(10_400)
    }

    fn address(&self) -> net::SocketAddr {
        if self.host_on_all_interfaces {
            ([0, 0, 0, 0], self.port()).into()
        } else {
            ([127, 0, 0, 1], self.port()).into()
        }
    }

    fn port(&self) -> u16 {
        self.port.unwrap_or(3030)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let options: Options = argh::from_env();

    let color_scale = Arc::new(ColorScale::from_file("./colors.txt", options.max_views()).await?);

    let svg_pattern = "$MARKER$";
    let svg_file =
        Arc::new(ViewCountSVG::from_file("./view_count_template.svg", svg_pattern).await?);

    let view_count_map: HashMap<String, u64> = HashMap::new();
    let view_count_map = Arc::new(Mutex::new(view_count_map));

    let view_count_map_view = warp::any().map(move || Arc::clone(&view_count_map));
    let svg_file_view = warp::any().map(move || Arc::clone(&svg_file));
    let color_scale_view = warp::any().map(move || Arc::clone(&color_scale));

    let index = warp::path::end().map(|| StatusCode::OK);

    let view_count_route = warp::path::param::<String>()
        .and(view_count_map_view)
        .and(svg_file_view)
        .and(color_scale_view)
        .and(warp::filters::query::query::<QueryParameters>())
        .map(
            |input: String,
             task_view_map: Arc<Mutex<HashMap<String, u64>>>,
             svg_file: Arc<ViewCountSVG>,
             color_scale: Arc<ColorScale>,
             options: QueryParameters| match task_view_map.lock() {
                Ok(mut gate) => {
                    let view_count = gate.entry(input).or_insert(0);
                    *view_count += 1;

                    let color = match options.fill_mode.unwrap_or(FillMode::MIlestone) {
                        FillMode::MIlestone => color_scale.hex_color_for_view_count(*view_count),
                        FillMode::Random => color_scale.random_hex_color(),
                    };

                    let returned_svg_content =
                        svg_file.template(&*format!("#{}", color), &*view_count.to_string());

                    Response::builder()
                        .header("Content-Type", "image/svg+xml; charset=utf-8")
                        .header(
                            "Cache-Control",
                            "max-age=0, no-cache, no-store, must-revalidate",
                        )
                        .status(StatusCode::OK)
                        .body(returned_svg_content)
                }
                Err(why) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(format!(
                        "Failed to calculate view count, try again: {}",
                        &why
                    )),
            },
        );

    warp::serve(index.or(view_count_route))
        .run(options.address())
        .await;

    Ok(())
}
