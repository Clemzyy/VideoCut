// Icône et informations de version de l'exécutable Windows.
fn main() {
    println!("cargo:rerun-if-changed=assets/Icone.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/Icone.ico")
            .set("ProductName", "VideoCut")
            .set("FileDescription", "VideoCut - découpeur audio/vidéo")
            .set("CompanyName", "Clemzy")
            .set("LegalCopyright", "© Clemzy - Licence MIT");
        if let Err(err) = resource.compile() {
            println!("cargo:warning=Icône de l'exécutable non intégrée : {err}");
        }
    }
}
