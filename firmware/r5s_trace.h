#ifndef R5S_TRACE_H
#define R5S_TRACE_H

#ifndef R5S_GPIO_BASE
#define R5S_GPIO_BASE 0xfe770000UL
#endif
#ifndef R5S_STOP_CHECKPOINT
#define R5S_STOP_CHECKPOINT 0
#endif
#ifndef R5S_WRITE
#define R5S_WRITE(value, address) (*(volatile unsigned int *)(address) = (value))
#endif
#ifndef R5S_WAIT
static inline unsigned long long r5s_ticks(void)
{
    unsigned long long value;
    __asm__ volatile("isb; mrs %0, cntvct_el0" : "=r"(value));
    return value;
}
static inline void r5s_wait(unsigned int ms)
{
    unsigned long long start = r5s_ticks();
    while (r5s_ticks() - start < (unsigned long long)ms * 24000)
        __asm__ volatile("yield");
}
#define R5S_WAIT(ms) r5s_wait(ms)
#endif
#ifndef R5S_BARRIER
#define R5S_BARRIER() __asm__ volatile("dsb sy" ::: "memory")
#endif

static inline void r5s_led(unsigned int on)
{
    R5S_WRITE(0x04000000u | (on ? 0x400u : 0), R5S_GPIO_BASE + 4);
    R5S_BARRIER();
}
static inline void r5s_pulses(unsigned int code)
{
    for (unsigned int i = 0; i < code; i++) {
        r5s_led(1); R5S_WAIT(150);
        r5s_led(0); R5S_WAIT(250);
    }
    R5S_WAIT(2000);
}
static inline void r5s_frame(unsigned int code)
{
    r5s_led(1); R5S_WAIT(1000);
    r5s_led(0); R5S_WAIT(500);
    r5s_pulses(code);
}
static inline void r5s_checkpoint(unsigned int code)
{
    do { r5s_frame(code); } while (R5S_STOP_CHECKPOINT == code);
}
static inline void r5s_error(unsigned int code)
{
    for (;;) {
        r5s_led(1); R5S_WAIT(1500);
        r5s_led(0); R5S_WAIT(500);
        r5s_led(1); R5S_WAIT(1500);
        r5s_led(0); R5S_WAIT(500);
        r5s_pulses(code);
    }
}
#endif
