use std::{env,path::PathBuf,process::Command};
fn main() {
 let root=PathBuf::from(env::var("R5S_PROJECT_ROOT").unwrap());let out=PathBuf::from(env::var("OUT_DIR").unwrap());
 let tx_checksum=env::var("R5S_WAN_TX_IPV4_CHECKSUM").as_deref()==Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_WAN_TX_IPV4_CHECKSUM");
 let rx_checksum=env::var("R5S_WAN_RX_CHECKSUM").as_deref()==Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_WAN_RX_CHECKSUM");
 let large_ring=env::var("R5S_GMAC_RING").as_deref()==Ok("256");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_RING");
 let dma_nc=env::var("R5S_GMAC_DMA_CACHE").as_deref()==Ok("nc");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_DMA_CACHE");

 let tso=env::var("R5S_WAN_TSO").as_deref()==Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_WAN_TSO");
 let pbl = env::var("R5S_GMAC_PBL").unwrap_or_else(|_| "8".into());
 assert!(["8", "16", "32"].contains(&pbl.as_str()));
 println!("cargo:rerun-if-env-changed=R5S_GMAC_PBL");
 let pblx8 = env::var("R5S_GMAC_PBLX8").as_deref() == Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_PBLX8");
 let rk3568_axi = env::var("R5S_GMAC_RK3568_AXI").as_deref() == Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_RK3568_AXI");
 let eame = env::var("R5S_GMAC_EAME").as_deref() == Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_EAME");
 let hybrid_irq = env::var("R5S_GMAC_HYBRID_IRQ").as_deref() == Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_HYBRID_IRQ");
 let a55_opt = env::var("R5S_A55_OPT").as_deref() == Ok("1");
 println!("cargo:rerun-if-env-changed=R5S_A55_OPT");
 let raw=env::var_os("CARGO_FEATURE_RAWIO").is_some();
 let writeback=env::var("R5S_ZC_CACHE").as_deref()==Ok("writeback") && env::var_os("CARGO_FEATURE_ZC").is_some();
 println!("cargo:rerun-if-env-changed=R5S_ZC_CACHE");
 let two_rx=env::var("R5S_GMAC_RX_CHANNELS").as_deref()==Ok("2");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_RX_CHANNELS");
 let two_tx=env::var("R5S_GMAC_TX_CHANNELS").as_deref()==Ok("2");
 println!("cargo:rerun-if-env-changed=R5S_GMAC_TX_CHANNELS");
 let mut source=std::fs::read_to_string(root.join("drivers/gmac0/link.c")).unwrap();
 let anchor="int link_poll(unsigned *speed,unsigned *duplex) {";
 assert_eq!(source.matches(anchor).count(),1);
 let poll_path=if raw {"drivers/gmac0/negotiated.c.inc"} else {"pds/preflight_fault_id/src/poll.c"};
 let poll=std::fs::read_to_string(root.join(poll_path)).unwrap();
 source=format!("{}{}",source.split_once(anchor).unwrap().0,poll);
 if raw {source=source.replace("#include \"mdio.h\"","#include \"mdio.h\"\n#include \"dma/dma.h\"");}
 println!("cargo:rerun-if-changed={}",root.join(poll_path).display());
 if env::var_os("CARGO_FEATURE_LAN_GATED").is_some() {
  let leds=root.join("drivers/gmac0/jack-led.c.inc");
  source.push_str(&std::fs::read_to_string(&leds).unwrap());
  println!("cargo:rerun-if-changed={}",leds.display());
 }
 std::fs::write(out.join("link-profile.c"),source).unwrap();
 let mut objects=Vec::new();
 for (name,path) in [("mdio",root.join("drivers/gmac0/mdio.c")),("link",out.join("link-profile.c")),("dma",root.join("drivers/gmac0/dma/dma.c"))] {
 if name=="dma" && !raw {continue;}
 let object=out.join(format!("{name}.o"));
 let mut compiler=Command::new("aarch64-none-elf-gcc");
 if name=="dma" && rx_checksum {compiler.arg("-DR5S_WAN_RX_CHECKSUM");}
 if name=="dma" && tx_checksum {compiler.arg("-DR5S_WAN_TX_IPV4_CHECKSUM");}
 if name=="dma" && large_ring {compiler.arg("-DR5S_GMAC_RING_256");}
 if name=="dma" && dma_nc {compiler.arg("-DR5S_GMAC_DMA_NC");}
 if name=="dma" && tso {compiler.arg("-DR5S_WAN_TSO");}
 if name=="dma" && two_rx {compiler.arg("-DR5S_GMAC_TWO_RX");}
 if name=="dma" && two_tx {compiler.arg("-DR5S_GMAC_TWO_TX");}
 if name=="dma" && writeback {compiler.arg("-DR5S_ZC_WRITEBACK");}
 if name=="dma" {compiler.arg(format!("-DR5S_GMAC_PBL={pbl}"));}
 if name=="dma" && pblx8 {compiler.arg("-DR5S_GMAC_PBLX8");}
 if name=="dma" && rk3568_axi {compiler.arg("-DR5S_GMAC_RK3568_AXI");}
 if name=="dma" && eame {compiler.arg("-DR5S_GMAC_EAME");}
 if name=="dma" && hybrid_irq {compiler.arg("-DR5S_GMAC_HYBRID_IRQ");}
 assert!(compiler.args([if a55_opt {"-O3"} else {"-O2"},"-std=c11","-ffreestanding","-fno-stack-protector","-mcpu=cortex-a55","-Wall","-Wextra","-Werror","-I"]).arg(root.join("drivers/gmac0")).arg("-c").arg(&path).arg("-o").arg(&object).status().unwrap().success());objects.push(object);
 }
 assert!(Command::new("aarch64-none-elf-ar").arg("crs").arg(out.join("libgmac.a")).args(objects).status().unwrap().success());
 println!("cargo:rerun-if-changed={}",root.join("pds/preflight_fault_id/src/poll.c").display());
 println!("cargo:rustc-link-search=native={}",out.display());println!("cargo:rustc-link-lib=static=gmac");
 for name in ["link.c","link.h","mdio.c","mdio.h","link-config.h","dma/dma.c","dma/dma.h"] {println!("cargo:rerun-if-changed={}",root.join("drivers/gmac0").join(name).display());}
}
