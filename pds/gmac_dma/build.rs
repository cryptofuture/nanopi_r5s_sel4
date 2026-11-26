use std::{env,path::PathBuf,process::Command};
fn main() {
 let root=PathBuf::from(env::var("R5S_PROJECT_ROOT").unwrap());let out=PathBuf::from(env::var("OUT_DIR").unwrap());
 let raw=env::var_os("CARGO_FEATURE_RAWIO").is_some();
 let mut source=std::fs::read_to_string(root.join("drivers/gmac0/link.c")).unwrap();
 if raw {
 source=source.replace("#include \"mdio.h\"","#include \"mdio.h\"\n#include \"dma/dma.h\"");
 source=source.replace("    if (!stopped()) return -3;","    if (!dma_enabled() && ((hw_read(0,0)&3) || ((hw_read(0,0x1104)|hw_read(0,0x1108))&1))) return -3;");
 source=source.replace("    hw_write(3,0x17c,0x00300000|", "    int change=dma_prepare_link(*speed,*duplex);if (change) return change;\n    hw_write(3,0x17c,0x00300000|");
 source=source.replace("    hw_write(0,0,config);","    config|=dma_enabled()?3u:0;\n    hw_write(0,0,config);");
 }
 std::fs::write(out.join("link-profile.c"),source).unwrap();
 let mut objects=Vec::new();
 for (name,path) in [("mdio",root.join("drivers/gmac0/mdio.c")),("link",out.join("link-profile.c")),("dma",root.join("drivers/gmac0/dma/dma.c"))] {
 if name=="dma" && !raw {continue;}
 let object=out.join(format!("{name}.o"));
 assert!(Command::new("aarch64-none-elf-gcc").args(["-O2","-std=c11","-ffreestanding","-fno-stack-protector","-mcpu=cortex-a55","-Wall","-Wextra","-Werror","-I"]).arg(root.join("drivers/gmac0")).arg("-c").arg(&path).arg("-o").arg(&object).status().unwrap().success());objects.push(object);
 }
 assert!(Command::new("aarch64-none-elf-ar").arg("crs").arg(out.join("libgmac.a")).args(objects).status().unwrap().success());
 println!("cargo:rustc-link-search=native={}",out.display());println!("cargo:rustc-link-lib=static=gmac");
 for name in ["link.c","link.h","mdio.c","mdio.h","link-config.h","dma/dma.c","dma/dma.h"] {println!("cargo:rerun-if-changed={}",root.join("drivers/gmac0").join(name).display());}
}
