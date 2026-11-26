pub fn observed_link(status: Option<&[u32; 20]>, previous: bool) -> bool {
    status.map_or(previous, |sample| sample[2] == 2)
}
