use std::{env,path::PathBuf,process::Command};
fn main() {
 let root=PathBuf::from(env::var("R5S_PROJECT_ROOT").unwrap());
 let out=PathBuf::from(env::var("OUT_DIR").unwrap());
 for name in ["mdio","link"] {
  assert!(Command::new("aarch64-none-elf-gcc").args(["-O2","-std=c11","-ffreestanding","-fno-stack-protector","-mcpu=cortex-a55","-Wall","-Wextra","-Werror","-c"]).arg(root.join(format!("drivers/gmac0/{name}.c"))).arg("-o").arg(out.join(format!("{name}.o"))).status().unwrap().success());
  println!("cargo:rerun-if-changed={}",root.join(format!("drivers/gmac0/{name}.c")).display());
 }
 assert!(Command::new("aarch64-none-elf-ar").arg("crs").arg(out.join("libgmac_link.a")).arg(out.join("mdio.o")).arg(out.join("link.o")).status().unwrap().success());
 println!("cargo:rustc-link-search=native={}",out.display());println!("cargo:rustc-link-lib=static=gmac_link");
 println!("cargo:rerun-if-changed={}",root.join("drivers/gmac0/link-config.h").display());
 for name in ["link.h","mdio.h"] {println!("cargo:rerun-if-changed={}",root.join("drivers/gmac0").join(name).display());}
}
