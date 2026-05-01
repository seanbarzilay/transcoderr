pub mod browse;
pub mod flows;
pub mod notifiers;
pub mod plugins;
pub mod runs;
pub mod sources;
pub mod system;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Empty request object for tools that take no parameters. Using this
// instead of `()` keeps `inputSchema.type === "object"`, which Claude
// Code's Zod-based MCP validator requires (rmcp's `Parameters<()>`
// produces `{"const": null, "nullable": true, "title": "null"}`, which
// is rejected and silently drops the entire tool list). Plain `//` so
// the explanation doesn't leak into every tool's schema.description.
#[derive(Default, Deserialize, Serialize, JsonSchema)]
pub struct NoArgs {}
