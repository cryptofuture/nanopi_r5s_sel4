/* SPDX-License-Identifier: BSD-2-Clause */
int link_configure_jack_leds(void)
{
    const uint16_t fields = (0x1f << 5) | (0x1f << 10);
    const uint16_t mode_b = 1u << 15;
    const uint16_t activity = 1u << (5 + 4);
    const uint16_t all_link_rates = 0x0b << 10;
    uint16_t value, actual;
    if (link_phy_id != EXPECT_PHY_ID) return 0;
    int valid = !write_phy(31, 0xd04) && !read_phy(16, &value) && value != 0xffff;
    if (valid) {
        uint16_t wanted = (value & ~fields) | mode_b | activity | all_link_rates;
        valid = (value == wanted || !write_phy(16, wanted)) &&
                !read_phy(16, &actual) && actual == wanted;
    }
    return !write_phy(31, 0) && valid;
}
