# Corrected raw-I/O and WAN-DHCP images

Current delivery: [results](../out/network-corrected/RESULTS.md) and [operator instructions](../out/network-corrected/operator.md).

Active build commands after sourcing scripts/env: `python3 scripts/build-network-rawio`, then `python3 scripts/build-network-wan`. Outputs and source fingerprints live under out/network-corrected. Earlier raw-I/O/WAN artifacts remain preserved and do not contain this correction lineage.

User reported READY on the LDO_REG4-policy preflight. Its recovery result remains pending. New raw and WAN images are hardware-untested; raw proof and recovery must precede WAN promotion. DMA authority remains trusted, without demonstrated SMMU confinement.
