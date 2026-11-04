fn main() {
    println!("cargo:rerun-if-changed=res/prisma.ico");
    println!("cargo:rerun-if-changed=res/prisma.manifest");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("res/prisma.ico");
    res.set_manifest_file("res/prisma.manifest");
    res.compile().expect("falha ao compilar recursos");
}
