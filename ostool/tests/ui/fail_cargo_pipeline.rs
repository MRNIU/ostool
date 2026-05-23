use ostool::build::cargo_pipeline::CargoBuildPipeline;

fn main() {
    let _ = core::mem::size_of::<CargoBuildPipeline<'static>>();
}
