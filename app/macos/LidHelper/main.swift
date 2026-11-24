// Claktory's lid-closed helper (#371; design: coordination/security/lid-closed.md).
//
// A root launchd daemon, registered by Claktory's Mac app with SMAppService,
// whose whole power is one bit: whether the Mac may sleep (the system's
// SleepDisabled setting, which `pmset disablesleep` sets). factoryd, and
// only a factoryd signed by Claktory's own team, asks it over XPC to hold
// the Mac awake for a 10-minute lease while agents work; the helper lets go
// by itself when the lease runs out, the battery runs low, the Mac gets hot,
// or the lid is closed on battery (a laptop in a bag).
//
// It reads nothing it's given but its messages: no arguments, no
// environment, no file. It runs no other program. At every start it clears
// the setting, so a crash, a reboot or a deleted app never leaves a Mac
// that can't sleep.
//
// Contract (factoryd <-> helper), one XPC dictionary per request, integers
// and booleans only:
//   request:  "v" (contract version) and "op" (1 status, 2 hold, 3 release)
//   reply:    "v", "version" (this helper's build), "holding" (bool),
//             "limit" (0 none, 1 battery, 2 heat, 3 lid closed on battery),
//             "lease_ms_left"; or "error" (1: unknown version or request).

import Foundation
import IOKit
import IOKit.ps
import Security
import os

/// The contract's version: what factoryd and the helper must agree on.
let contract: Int64 = 1
/// This helper's own build; bumped with every change to this file.
let build: Int64 = 1
/// The Mach service launchd gives it (MachServices in its plist).
let service = "com.claktory.lid-helper"
/// The only client: factoryd, signed by the helper's own team.
let clientIdentifier = "dev.factory.factoryd"

/// How long a hold lasts from its last renewal.
let lease: TimeInterval = 600
/// How often the limits are looked at.
let every: TimeInterval = 30
/// On battery at or under this, it lets go; it holds again at `batteryRearm`
/// or on power.
let batteryFloor = 20
let batteryRearm = 30
/// Back at nominal or fair this long after serious or critical heat.
let coolFor: TimeInterval = 600
/// Nothing held for this long: it exits; launchd starts it again on demand.
let idleExit: TimeInterval = 60

let log = Logger(subsystem: service, category: "helper")

enum Limit: Int64 {
    case none = 0, battery = 1, heat = 2, lidOnBattery = 3
}

// MARK: The machine

/// IOPMSetSystemPowerSetting, which pmset uses for disablesleep; found by name.
typealias SetSetting = @convention(c) (CFString, CFTypeRef) -> IOReturn
let setSetting: SetSetting? = {
    guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "IOPMSetSystemPowerSetting") else { return nil }
    return unsafeBitCast(symbol, to: SetSetting.self)
}()

/// Allows sleep (false) or not (true); whether it took.
@discardableResult
func sleepDisabled(_ on: Bool) -> Bool {
    guard let setSetting else {
        log.error("IOPMSetSystemPowerSetting isn't there")
        return false
    }
    let result = setSetting("SleepDisabled" as CFString, (on ? kCFBooleanTrue : kCFBooleanFalse)!)
    if result != kIOReturnSuccess {
        log.error("could not set SleepDisabled: \(result, privacy: .public)")
    }
    return result == kIOReturnSuccess
}

struct Power {
    var plugged: Bool
    /// Percent, or nil without a battery.
    var battery: Int?
}

func power() -> Power {
    // Unreadable: on battery, at an unknown level, so the battery limit
    // applies (fails closed).
    guard let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue() else { return Power(plugged: false, battery: 0) }
    let providing = IOPSGetProvidingPowerSourceType(info)?.takeUnretainedValue() as String?
    var battery: Int?
    if let list = IOPSCopyPowerSourcesList(info)?.takeRetainedValue() as? [CFTypeRef] {
        for source in list {
            guard let description = IOPSGetPowerSourceDescription(info, source)?.takeUnretainedValue() as? [String: Any],
                  let current = description[kIOPSCurrentCapacityKey] as? Int,
                  let max = description[kIOPSMaxCapacityKey] as? Int, max > 0
            else { continue }
            battery = current * 100 / max
        }
    }
    return Power(plugged: providing == kIOPMACPowerKey, battery: battery)
}

/// Whether a laptop's lid is closed.
func lidClosed() -> Bool {
    let root = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOPMrootDomain"))
    guard root != 0 else { return false }
    defer { IOObjectRelease(root) }
    let state = IORegistryEntryCreateCFProperty(root, "AppleClamshellState" as CFString, kCFAllocatorDefault, 0)?.takeRetainedValue()
    return (state as? Bool) ?? false
}

func hot() -> Bool {
    let state = ProcessInfo.processInfo.thermalState
    return state == .serious || state == .critical
}

// MARK: The hold

final class Hold {
    private var leaseUntil: Date?
    private var held = false
    private var batteryLow = false
    private var heatSince: Date?
    private var cooledSince: Date?
    private var idleSince = Date()

    /// The limit that applies now, its re-arming included.
    private func limit() -> Limit {
        let now = Date()
        let power = power()
        if power.plugged {
            batteryLow = false
        } else if let battery = power.battery {
            if battery <= batteryFloor { batteryLow = true }
            if battery >= batteryRearm { batteryLow = false }
        }
        if hot() {
            heatSince = heatSince ?? now
            cooledSince = nil
        } else if heatSince != nil {
            cooledSince = cooledSince ?? now
            if let cooled = cooledSince, now.timeIntervalSince(cooled) >= coolFor {
                heatSince = nil
                cooledSince = nil
            }
        }
        if !power.plugged && lidClosed() { return .lidOnBattery }
        if heatSince != nil { return .heat }
        if batteryLow { return .battery }
        return .none
    }

    /// Looks again: lets go on a limit or a lease run out.
    func check() -> Limit {
        let limit = limit()
        let leased = leaseUntil.map { $0 > Date() } ?? false
        let hold = leased && limit == .none
        if !leased { leaseUntil = nil }
        if hold != held {
            held = hold && sleepDisabled(true)
            if !held { sleepDisabled(false) }
            log.info("holding: \(self.held, privacy: .public), limit: \(limit.rawValue, privacy: .public)")
        }
        if held || leaseUntil != nil {
            idleSince = Date()
        } else if Date().timeIntervalSince(idleSince) >= idleExit {
            log.info("nothing held: exiting")
            exit(0)
        }
        return limit
    }

    func hold() -> Limit {
        if limit() == .none { leaseUntil = Date().addingTimeInterval(lease) }
        return check()
    }

    func release() -> Limit {
        leaseUntil = nil
        return check()
    }

    func reply(to request: xpc_object_t, limit: Limit) -> xpc_object_t {
        let reply = xpc_dictionary_create_reply(request) ?? xpc_dictionary_create(nil, nil, 0)
        xpc_dictionary_set_int64(reply, "v", contract)
        xpc_dictionary_set_int64(reply, "version", build)
        xpc_dictionary_set_bool(reply, "holding", held)
        xpc_dictionary_set_int64(reply, "limit", limit.rawValue)
        let left = leaseUntil.map { max(0, $0.timeIntervalSinceNow) } ?? 0
        xpc_dictionary_set_int64(reply, "lease_ms_left", Int64(left * 1000))
        return reply
    }
}

// MARK: Who may ask

/// The team that signed this helper; its client must be signed by it too.
func ownTeam() -> String? {
    var code: SecCode?
    guard SecCodeCopySelf([], &code) == errSecSuccess, let code else { return nil }
    var staticCode: SecStaticCode?
    guard SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode else { return nil }
    var information: CFDictionary?
    guard SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &information) == errSecSuccess,
          let information = information as? [String: Any]
    else { return nil }
    let team = information[kSecCodeInfoTeamIdentifier as String] as? String
    // Only letters and digits: it goes in a requirement.
    return team.flatMap { $0.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber) } && !$0.isEmpty ? $0 : nil }
}

// MARK: Start

let queue = DispatchQueue(label: service)
let holding = Hold()

// Safe at start: whatever was left set, the Mac may sleep again.
sleepDisabled(false)

guard let team = ownTeam() else {
    // A successful exit: launchd doesn't start it again and again.
    log.error("not signed by a team: refusing everyone")
    exit(0)
}
let requirement = "anchor apple generic and certificate leaf[subject.OU] = \"\(team)\" and identifier \"\(clientIdentifier)\""

let listener = xpc_connection_create_mach_service(service, queue, UInt64(XPC_CONNECTION_MACH_SERVICE_LISTENER))
xpc_connection_set_event_handler(listener) { peer in
    guard xpc_get_type(peer) == XPC_TYPE_CONNECTION else { return }
    // Checked by XPC against the peer's audit token, before any message.
    guard xpc_connection_set_peer_code_signing_requirement(peer, requirement) == 0 else {
        xpc_connection_cancel(peer)
        return
    }
    xpc_connection_set_event_handler(peer) { request in
        guard xpc_get_type(request) == XPC_TYPE_DICTIONARY else { return }
        let v = xpc_dictionary_get_int64(request, "v")
        let op = xpc_dictionary_get_int64(request, "op")
        // Exactly "v" and "op", both integers: no strings, nothing more,
        // and so far under 1 KB.
        var keys = 0
        var integers = true
        xpc_dictionary_apply(request) { _, value in
            keys += 1
            integers = integers && xpc_get_type(value) == XPC_TYPE_INT64
            return keys <= 2
        }
        guard v == contract, keys == 2, integers, (1...3).contains(op) else {
            let reply = xpc_dictionary_create_reply(request) ?? xpc_dictionary_create(nil, nil, 0)
            xpc_dictionary_set_int64(reply, "error", 1)
            xpc_connection_send_message(peer, reply)
            return
        }
        let limit: Limit
        switch op {
        case 2: limit = holding.hold()
        case 3: limit = holding.release()
        default: limit = holding.check()
        }
        xpc_connection_send_message(peer, holding.reply(to: request, limit: limit))
    }
    // Its requests one at a time, on the same queue as the timer's checks.
    xpc_connection_set_target_queue(peer, queue)
    xpc_connection_activate(peer)
}
xpc_connection_activate(listener)

let timer = DispatchSource.makeTimerSource(queue: queue)
timer.schedule(deadline: .now() + every, repeating: every)
timer.setEventHandler { _ = holding.check() }
timer.activate()

dispatchMain()
