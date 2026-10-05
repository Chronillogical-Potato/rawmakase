//! MCP is a protocol adapter: all editing stays in the running Editor.
use crate::control_client::{Connection, default_data_dir};
use base64::{Engine, engine::general_purpose::STANDARD};
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(clap::Args)]
pub struct Cli {
    /// Data folder of the running app; defaults to RAWMAKASE_DATA_DIR or the platform default.
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Target {
    /// Catalog photo ID from get_state (omit only for an uncataloged photo).
    photo_id: Option<i64>,
    /// Document generation from the latest state.
    generation: u64,
    /// Recipe revision from the latest state; refresh after every edit.
    revision: u64,
    /// Optional mask index from state. Omit for global edits.
    mask: Option<usize>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PhotoQuery {
    /// Filename substring; omit to list the catalog.
    query: Option<String>,
    offset: Option<usize>,
    /// At most 500 photos; defaults to 100.
    limit: Option<usize>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenPhoto {
    id: i64,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Parameter {
    /// Parameter name from get_capabilities, e.g. exposure or band3.sat.
    param: String,
    /// Absolute value in the parameter's displayed units.
    value: f32,
    target: Target,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Action {
    /// Named action from get_capabilities, e.g. undo, redo, treatment:bw or rating:3.
    action: String,
    target: Target,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Guarded {
    target: Target,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Preview {
    target: Target,
    /// Long edge in pixels (16 through 2048); defaults to 1600.
    max_edge: Option<u32>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Export {
    /// Absolute path for a new JPEG or TIFF. Existing files are never replaced.
    path: String,
    target: Target,
    /// Optional maximum edge; omit for full size.
    max_edge: Option<u32>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Job {
    job_id: u64,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CurveChannel {
    Rgb,
    Red,
    Green,
    Blue,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ToneCurve {
    channel: CurveChannel,
    /// 2–32 [input, output] points in 0–1, with strictly increasing inputs at least 0.00049 apart. Example gentle S: [[0,0],[0.25,0.2],[0.75,0.8],[1,1]].
    points: Vec<[f32; 2]>,
    target: Target,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CurvePreset {
    Linear,
    MediumContrast,
    StrongContrast,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Preset {
    preset: CurvePreset,
    target: Target,
}

#[derive(Clone)]
struct Server {
    data_dir: PathBuf,
    workers: Arc<tokio::sync::Semaphore>,
}

fn error(code: &str, message: impl Into<String>) -> CallToolResult {
    CallToolResult::structured_error(json!({"ok":false,"code":code,"error":message.into()}))
}
fn result(reply: Value) -> CallToolResult {
    if reply["ok"] == true {
        CallToolResult::structured(reply)
    } else {
        CallToolResult::structured_error(reply)
    }
}
fn ask(connection: &Connection, request: Value) -> Result<Value, CallToolResult> {
    let reply = connection
        .request(request)
        .map_err(|e| error("connection_error", e))?;
    if reply["ok"] == true {
        Ok(reply)
    } else {
        Err(result(reply))
    }
}

impl Server {
    async fn work(
        &self,
        task: impl FnOnce(Connection) -> CallToolResult + Send + 'static,
    ) -> CallToolResult {
        let Ok(permit) = self.workers.clone().try_acquire_owned() else {
            return error(
                "busy",
                "Four tool calls are already running; wait for one to finish",
            );
        };
        let dir = self.data_dir.clone();
        // Blocking socket I/O never blocks MCP's reader or cancellation handling.
        // Keep the permit in the worker even if the client cancels its wait.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match Connection::read(&dir) {
                Ok(connection) => task(connection),
                Err(e) => error("app_unavailable", e),
            }
        })
        .await
        .unwrap_or_else(|e| error("adapter_error", e.to_string()))
    }
    async fn send(&self, request: Value) -> CallToolResult {
        self.work(move |connection| match ask(&connection, request) {
            Ok(reply) => result(reply),
            Err(e) => e,
        })
        .await
    }
}

#[tool_router]
impl Server {
    fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            workers: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
    #[tool(
        description = "Read the app state, current photo, recipe revision, parameters, masks and loading/saving state.",
        annotations(read_only_hint = true)
    )]
    async fn get_state(&self) -> CallToolResult {
        self.send(json!({"cmd":"state"})).await
    }
    #[tool(
        description = "Discover supported actions, parameter names, units, ranges and mask support.",
        annotations(read_only_hint = true)
    )]
    async fn get_capabilities(&self) -> CallToolResult {
        self.send(json!({"cmd":"capabilities"})).await
    }
    #[tool(
        description = "Find catalog photos by filename and return IDs for open_photo.",
        annotations(read_only_hint = true)
    )]
    async fn find_photos(&self, Parameters(p): Parameters<PhotoQuery>) -> CallToolResult {
        self.send(json!({"cmd":"photos","query":p.query.unwrap_or_default(),"offset":p.offset.unwrap_or(0),"limit":p.limit.unwrap_or(100)})).await
    }
    #[tool(
        description = "Open a catalog photo in Develop and wait up to 30 seconds for decoding. Fails if another photo replaces it. Opening may save the previous edit."
    )]
    async fn open_photo(
        &self,
        Parameters(p): Parameters<OpenPhoto>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.work(move |c| {
            if context.ct.is_cancelled() { return error("cancelled", "Tool call was cancelled"); }
            let opened = match ask(&c, json!({"cmd":"open","id":p.id})) { Ok(r) => r, Err(e) => return e };
            let generation = opened["state"]["generation"].clone();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if context.ct.is_cancelled() { return error("cancelled", "Stopped waiting; opening may already have applied"); }
                let reply = match ask(&c, json!({"cmd":"state"})) { Ok(r) => r, Err(e) => return e };
                let state = &reply["state"];
                if state["photo_id"].as_i64().is_some_and(|id| id != p.id) || state["generation"] != generation || state["mode"] != "develop" {
                    return error("stale_target", "Another document replaced the requested photo; read state");
                }
                if state["loaded"] == true && state["photo_id"].as_i64() == Some(p.id) { return result(reply); }
                if Instant::now() >= deadline { return error("not_ready", "Opening started but decoding has not completed; inspect state before editing"); }
                std::thread::sleep(Duration::from_millis(100));
            }
        }).await
    }
    #[tool(
        description = "Set one parameter in displayed units. Requires current generation/revision; mask scope is optional. Returns updated state and guards. Values are clamped to supported ranges."
    )]
    async fn set_parameter(&self, Parameters(p): Parameters<Parameter>) -> CallToolResult {
        self.send(json!({"cmd":"set","param":p.param,"value":p.value,"target":p.target}))
            .await
    }
    #[tool(
        description = "Run a named application action, including undo/redo, treatment, rating or reset. Requires fresh target guards. Some actions open dialogs; prefer export_photo for unattended export. Never blindly retry toggle/relative actions."
    )]
    async fn run_action(&self, Parameters(p): Parameters<Action>) -> CallToolResult {
        self.send(json!({"cmd":"action","action":p.action,"target":p.target}))
            .await
    }
    #[tool(
        description = "Replace the RGB, red, green or blue point curve with a natural cubic curve. Coordinates are normalized 0–1. Changes one channel as one undo step, preserving all other curves. Read get_state tone_curve before editing."
    )]
    async fn set_tone_curve(&self, Parameters(p): Parameters<ToneCurve>) -> CallToolResult {
        self.send(json!({"cmd":"curve","channel":p.channel,"points":p.points,"target":p.target}))
            .await
    }
    #[tool(
        description = "Apply a built-in RGB tone curve: linear, medium_contrast or strong_contrast. Preserves individual red, green and blue curves."
    )]
    async fn apply_curve_preset(&self, Parameters(p): Parameters<Preset>) -> CallToolResult {
        let action = match p.preset {
            CurvePreset::Linear => "curve:linear",
            CurvePreset::MediumContrast => "curve:medium_contrast",
            CurvePreset::StrongContrast => "curve:strong_contrast",
        };
        self.send(json!({"cmd":"action","action":action,"target":p.target}))
            .await
    }
    #[tool(
        description = "Start automatic tone adjustment for the current photo. Poll get_state until auto_running is false, then inspect the values and preview the result. This reply confirms starting, not successful completion."
    )]
    async fn auto_tone(&self, Parameters(p): Parameters<Guarded>) -> CallToolResult {
        self.send(json!({"cmd":"action","action":"auto_tone","target":p.target}))
            .await
    }
    #[tool(
        description = "Start automatic white balance. Poll get_state until auto_running is false, then inspect temperature/tint and preview. This reply confirms starting, not successful completion."
    )]
    async fn auto_white_balance(&self, Parameters(p): Parameters<Guarded>) -> CallToolResult {
        self.send(json!({"cmd":"action","action":"auto_white_balance","target":p.target}))
            .await
    }
    #[tool(
        description = "Save the current Develop edit to its catalog. Success confirms persistence; protected edits return an error."
    )]
    async fn save_photo(&self, Parameters(p): Parameters<Guarded>) -> CallToolResult {
        self.send(json!({"cmd":"save","target":p.target})).await
    }
    #[tool(
        description = "Render the guarded edit and return a JPEG image for visual inspection, plus its captured revision. Waits up to 120 seconds. Uses a temporary file, with no user output path."
    )]
    async fn preview_photo(
        &self,
        Parameters(p): Parameters<Preview>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let edge = p.max_edge.unwrap_or(1600);
        if !(16..=2048).contains(&edge) {
            return error("invalid_request", "max_edge must be between 16 and 2048");
        }
        self.work(move |c| preview(&c, p.target, edge, || context.ct.is_cancelled()))
            .await
    }
    #[tool(
        description = "Start exporting the guarded edit to a new JPEG/TIFF file. Returns a job ID; call get_job until status is completed before claiming the file exists. Does not overwrite files."
    )]
    async fn export_photo(&self, Parameters(p): Parameters<Export>) -> CallToolResult {
        let mut request = json!({"cmd":"export","path":p.path,"target":p.target});
        if let Some(edge) = p.max_edge {
            request["max_edge"] = edge.into();
        }
        self.send(request).await
    }
    #[tool(
        description = "Read an export job's progress and captured revision. Only completed confirms file publication; failed includes an error. Job IDs belong to the running app session.",
        annotations(read_only_hint = true)
    )]
    async fn get_job(&self, Parameters(p): Parameters<Job>) -> CallToolResult {
        self.send(json!({"cmd":"job","job_id":p.job_id})).await
    }
}

fn preview(
    c: &Connection,
    target: Target,
    edge: u32,
    cancelled: impl Fn() -> bool,
) -> CallToolResult {
    if cancelled() {
        return error("cancelled", "Tool call was cancelled");
    }
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => return error("preview_failed", e.to_string()),
    };
    let path = dir.path().join("preview.jpg");
    let started = match ask(
        c,
        json!({"cmd":"preview","path":path,"max_edge":edge,"target":target}),
    ) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Some(id) = started["result"]["job_id"].as_u64() else {
        return error("invalid_reply", "Preview returned no job ID");
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if cancelled() {
            return error(
                "cancelled",
                "Stopped waiting; temporary preview output will be discarded",
            );
        }
        let reply = match ask(c, json!({"cmd":"job","job_id":id})) {
            Ok(r) => r,
            Err(e) => return e,
        };
        match reply["result"]["status"].as_str() {
            Some("completed") => {
                let bytes = match std::fs::read(&path) {
                    Ok(b) if b.len() <= 8 * 1024 * 1024 => b,
                    Ok(_) => return error("preview_too_large", "Preview exceeds 8 MiB"),
                    Err(e) => return error("preview_failed", e.to_string()),
                };
                let mut output = result(reply);
                output
                    .content
                    .push(ContentBlock::image(STANDARD.encode(bytes), "image/jpeg"));
                return output;
            }
            Some("failed") => return CallToolResult::structured_error(reply),
            Some("running") => {}
            _ => return error("invalid_reply", "Unknown preview job status"),
        }
        if Instant::now() >= deadline {
            return error(
                "timeout",
                "Preview has not completed; its temporary output will be discarded",
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rawmakase", env!("CARGO_PKG_VERSION")))
            .with_instructions("Controls the running RAWmakase desktop app. Enable local scripts in Preferences > Automation. Read capabilities and state first. Use fresh generation/revision guards for each edit. Preview images to assess changes. Tool success confirms application, not save/export completion unless explicitly stated. Do not retry mutations after connection errors or outcome_unknown: inspect state. The desktop must remain responsive; this is not a headless service.")
    }
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let server = Server::new(cli.data_dir.unwrap_or_else(default_data_dir));
        let service = server.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}
