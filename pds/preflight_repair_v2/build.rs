use std::{env,path::PathBuf,process::Command};
fn main() {
 if env::var_os("CARGO_FEATURE_SUPERVISOR").is_some() {return;}
 let root=PathBuf::from(env::var("R5S_PROJECT_ROOT").unwrap());let out=PathBuf::from(env::var("OUT_DIR").unwrap());
 let original=std::fs::read_to_string(root.join("drivers/gmac0/link.c")).unwrap();let anchor="int link_poll(unsigned *speed,unsigned *duplex) {";
 assert_eq!(original.matches(anchor).count(),1);let prefix=original.split_once(anchor).unwrap().0;
 let fixed=root.join("pds/preflight_repair_v2/src/poll.c");std::fs::write(out.join("link-fixed.c"),format!("{}{}",prefix,std::fs::read_to_string(&fixed).unwrap())).unwrap();
 let mut objects=Vec::new();
 for (name,path) in [("link",out.join("link-fixed.c")),("mdio",root.join("drivers/gmac0/mdio.c"))] {
  let object=out.join(format!("{name}.o"));assert!(Command::new("aarch64-none-elf-gcc").args(["-O2","-std=c11","-ffreestanding","-fno-stack-protector","-mcpu=cortex-a55","-Wall","-Wextra","-Werror","-fstack-usage","-I"]).arg(root.join("drivers/gmac0")).arg("-c").arg(path).arg("-o").arg(&object).status().unwrap().success());objects.push(object);
 }
 assert!(Command::new("aarch64-none-elf-ar").arg("crs").arg(out.join("liblink_fixed.a")).args(objects).status().unwrap().success());
 println!("cargo:rustc-link-search=native={}",out.display());println!("cargo:rustc-link-lib=static=link_fixed");
 for path in [root.join("drivers/gmac0/link.c"),root.join("drivers/gmac0/link.h"),root.join("drivers/gmac0/mdio.h"),root.join("drivers/gmac0/link-config.h"),root.join("drivers/gmac0/mdio.c"),fixed] {println!("cargo:rerun-if-changed={}",path.display());}
}
