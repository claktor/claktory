// factoryd's side of the lid-closed helper (#371): one request over XPC to
// the helper's privileged Mach service, answered synchronously. The helper
// must be Claktory's own (our team, com.claktory.lid-helper), checked by
// XPC against its audit token, so a status shown always comes from it.
// Called only from factoryd's serve path (crate::keep_awake::lid).

#include <Security/Security.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <xpc/xpc.h>

struct claktory_lid_reply {
    int64_t contract;
    int64_t version;
    int64_t limit;
    int64_t lease_ms_left;
    int64_t error;
    int32_t holding;
};

// The team that signed this factoryd, letters and digits only; false
// without one (ad hoc: a dev or QA build, which no helper serves).
static int own_team(char *team, size_t size) {
    SecCodeRef code = NULL;
    SecStaticCodeRef static_code = NULL;
    CFDictionaryRef information = NULL;
    int found = 0;
    if (SecCodeCopySelf(kSecCSDefaultFlags, &code) == errSecSuccess &&
        SecCodeCopyStaticCode(code, kSecCSDefaultFlags, &static_code) == errSecSuccess &&
        SecCodeCopySigningInformation(static_code, kSecCSSigningInformation, &information) == errSecSuccess) {
        CFStringRef value = CFDictionaryGetValue(information, kSecCodeInfoTeamIdentifier);
        if (value != NULL && CFGetTypeID(value) == CFStringGetTypeID() &&
            CFStringGetCString(value, team, (CFIndex)size, kCFStringEncodingASCII) && team[0] != 0) {
            found = 1;
            for (const char *c = team; *c; c++) {
                if (!((*c >= 'A' && *c <= 'Z') || (*c >= 'a' && *c <= 'z') || (*c >= '0' && *c <= '9'))) {
                    found = 0;
                }
            }
        }
    }
    if (information) CFRelease(information);
    if (static_code) CFRelease(static_code);
    if (code) CFRelease(code);
    return found;
}

// 0: answered (see reply.error); -1: no helper answered; -2: this factoryd
// isn't signed by a team, so no helper of ours could be checked.
int claktory_lid_call(int64_t contract, int64_t op, struct claktory_lid_reply *reply) {
    memset(reply, 0, sizeof(*reply));
    char team[32];
    if (!own_team(team, sizeof(team))) {
        return -2;
    }
    char requirement[256];
    int written = snprintf(requirement, sizeof(requirement),
                           "anchor apple generic and certificate leaf[subject.OU] = \"%s\" and identifier \"com.claktory.lid-helper\"",
                           team);
    if (written <= 0 || (size_t)written >= sizeof(requirement)) {
        return -2;
    }
    xpc_connection_t connection =
        xpc_connection_create_mach_service("com.claktory.lid-helper", NULL, XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
    if (connection == NULL) {
        return -1;
    }
    if (xpc_connection_set_peer_code_signing_requirement(connection, requirement) != 0) {
        xpc_release(connection);
        return -2;
    }
    xpc_connection_set_event_handler(connection, ^(xpc_object_t event) {
      (void)event;
    });
    xpc_connection_activate(connection);
    xpc_object_t message = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_int64(message, "v", contract);
    xpc_dictionary_set_int64(message, "op", op);
    xpc_object_t answer = xpc_connection_send_message_with_reply_sync(connection, message);
    xpc_release(message);
    int result = -1;
    if (xpc_get_type(answer) == XPC_TYPE_DICTIONARY) {
        reply->error = xpc_dictionary_get_int64(answer, "error");
        reply->contract = xpc_dictionary_get_int64(answer, "v");
        reply->version = xpc_dictionary_get_int64(answer, "version");
        reply->limit = xpc_dictionary_get_int64(answer, "limit");
        reply->lease_ms_left = xpc_dictionary_get_int64(answer, "lease_ms_left");
        reply->holding = xpc_dictionary_get_bool(answer, "holding") ? 1 : 0;
        result = 0;
    }
    xpc_release(answer);
    xpc_connection_cancel(connection);
    xpc_release(connection);
    return result;
}
