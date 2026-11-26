#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <setjmp.h>
#include <string.h>

#define __iomem
struct cmd_tbl { int unused; };
#define U_BOOT_CMD(name, maximum, repeat, command, usage, help) \
    static const char registered_name[] = #name; \
    static int (*registered)(struct cmd_tbl *, int, int, char *const[]) = command; \
    _Static_assert(maximum == 1 && repeat == 0, "command registration changed")

static jmp_buf finish;
static struct { uintptr_t address; uint32_t value; } writes[32];
static unsigned delays[9], states[9], count, written;
static uint32_t state;

static void dsb(void) {}
static void writel(uint32_t value, void *address)
{
    assert(written < 32);
    writes[written].address = (uintptr_t)address;
    writes[written++].value = value;
    if ((uintptr_t)address == 0xfe770004)
        state = value;
}

void udelay(unsigned long usec)
{
    assert(count < 9);
    delays[count] = usec;
    states[count++] = state;
    if (count == 9)
        longjmp(finish, 1);
}

#include "../firmware/r5sboot.c"

int main(void)
{
    assert(strcmp(registered_name, "r5sboot") == 0);
    if (!setjmp(finish)) {
        registered(0, 0, 0, 0);
        assert(!"diagnostic returned");
    }
    const unsigned expected[] = {1000000, 750000, 250000, 750000, 2250000,
                                750000, 250000, 750000, 2250000};
    assert(memcmp(delays, expected, sizeof(expected)) == 0);
    for (unsigned i = 0; i < 9; i++)
        assert(states[i] == (0x04000000u | (i % 2 ? 0x400u : 0)));
    assert(writes[0].address == 0xfdd2037c && writes[0].value == (3u << 24));
    assert(writes[1].address == 0xfdd20458 && writes[1].value == (3u << 16));
    assert(writes[2].address == 0xfdc60078 && writes[2].value == (15u << 24));
    assert(writes[3].address == 0xfe770004 && writes[3].value == (1u << 26));
    assert(writes[4].address == 0xfe77000c && writes[4].value == ((1u << 26) | (1u << 10)));
    for (unsigned i = 5; i < written; i++) {
        assert(writes[i].address == 0xfe770004);
        assert((writes[i].value & ~((1u << 26) | (1u << 10))) == 0);
    }
    return 0;
}
