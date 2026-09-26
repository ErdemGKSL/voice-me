fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("voice-me.rc", embed_resource::NONE)
            .manifest_required()
            .expect("could not embed the Voice Me Windows icon");
    }
}
