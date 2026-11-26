// The kernel's power manager, as far as a laptop's lid goes (#371): the
// bit that keeps a closed lid from sleeping the Mac (IOPMrootDomain's
// kPMSetClamshellSleepState, which the person's own processes may set; it
// lives in the kernel's memory, and a restart clears it), and the thermal
// pressure level.

#include <IOKit/IOKitLib.h>
#include <notify.h>
#include <stdint.h>

// kPMSetClamshellSleepState in IOKit's IOPMLibDefs.h.
#define CLAKTORY_SET_CLAMSHELL_SLEEP_STATE 12

// Sets (1) or clears (0) the bit; 0, or the kern_return_t that refused it.
int claktory_clamshell_sleep_disable(int disable) {
    io_service_t root = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPMrootDomain"));
    if (root == IO_OBJECT_NULL) {
        return -1;
    }
    io_connect_t connection = IO_OBJECT_NULL;
    kern_return_t result = IOServiceOpen(root, mach_task_self(), 0, &connection);
    IOObjectRelease(root);
    if (result != KERN_SUCCESS) {
        return result;
    }
    uint64_t value = disable ? 1 : 0;
    result = IOConnectCallScalarMethod(connection, CLAKTORY_SET_CLAMSHELL_SLEEP_STATE, &value, 1, NULL, NULL);
    IOServiceClose(connection);
    return result;
}

// The thermal pressure level (0 nominal, 1 moderate, 2 heavy, 3 trapping,
// 4 sleeping), or -1 when it can't be read.
int claktory_thermal_pressure(void) {
    int token = 0;
    if (notify_register_check("com.apple.system.thermalpressurelevel", &token) != NOTIFY_STATUS_OK) {
        return -1;
    }
    uint64_t state = 0;
    uint32_t read = notify_get_state(token, &state);
    notify_cancel(token);
    return read == NOTIFY_STATUS_OK ? (int)state : -1;
}
