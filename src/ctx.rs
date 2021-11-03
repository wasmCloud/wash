use crate::util::Result;
use crate::util::{
    convert_rpc_error, extract_arg_value, format_output, json_str_to_msgpack_bytes,
    msgpack_to_json_val, nats_client_from_opts, Output, OutputKind,
};
use serde_json::json;
use std::path::PathBuf;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use wasmbus_rpc::{core::WasmCloudEntity, Message, RpcClient};
use wasmcloud_test_util::testing::TestResults;

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "ctx")]
pub(crate) struct CtxCli {
    #[structopt(flatten)]
    command: CtxCommand,
}

impl CtxCli {
    pub(crate) fn command(self) -> CtxCommand {
        self.command
    }
}

pub(crate) async fn handle_command(cmd: CtxCommand) -> Result<String> {
    let output_kind = cmd.output.kind;
    Ok(ctx_output())
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct CtxCommand {
    #[structopt(flatten)]
    pub(crate) output: Output,
}

pub(crate) async fn handle_ctx(cmd: CtxCommand) -> Result<Vec<u8>> {

}

// Helper output functions, used to ensure consistent output between ctl & standalone commands
pub(crate) fn call_output() -> String {

    "sup".to_string()
}

// #[cfg(test)]
// mod test {

// }
