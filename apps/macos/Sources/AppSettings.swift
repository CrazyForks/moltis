import Combine
import Foundation

final class AppSettings: ObservableObject {
    @Published var identityName = "Moltis"
    @Published var identityEmoji = ""
    @Published var identityTheme = ""
    @Published var identityUserName = ""
    @Published var identitySoul = ""

    @Published var environmentConfigDir = ""
    @Published var environmentDataDir = ""

    @Published var memoryEnabled = true
    @Published var memoryMode = "workspace"

    @Published var notificationsEnabled = true
    @Published var notificationsSoundEnabled = false

    @Published var cronJobs: [CronJobItem] = []
    @Published var heartbeatEnabled = true
    @Published var heartbeatIntervalMinutes = 5

    @Published var requirePassword = true
    @Published var passkeysEnabled = true
    @Published var tailscaleEnabled = false
    @Published var tailscaleMode = "off"

    @Published var channels: [ChannelItem] = []
    @Published var hooks: [HookItem] = []

    @Published var llmProvider = "openai"
    @Published var llmModel = "gpt-4.1"
    @Published var llmApiKey = ""

    @Published var mcpServers: [McpServerItem] = []
    @Published var skillPacks: [SkillPackItem] = []

    @Published var voiceEnabled = false
    @Published var voiceProvider = "none"
    @Published var voiceApiKey = ""

    @Published var terminalEnabled = false
    @Published var terminalShell = "/bin/zsh"

    @Published var sandboxEnabled = false
    @Published var containerImage = ""
    @Published var debugEnabled = false

    @Published var sandboxBackend = "auto"
    @Published var sandboxImage = "moltis/sandbox:latest"

    @Published var monitoringEnabled = true
    @Published var metricsEnabled = true
    @Published var tracingEnabled = true

    @Published var logLevel = "info"
    @Published var persistLogs = true

    @Published var graphqlEnabled = false
    @Published var graphqlPath = "/graphql"

    @Published var httpdEnabled = false
    @Published var httpdBindMode = "loopback"
    @Published var httpdPort = "8080"

    let httpdBindModes = ["loopback", "all"]

    @Published var configurationToml = ""

    let memoryModes = ["workspace", "global", "off"]
    let sandboxBackends = ["auto", "docker", "apple-container"]
    let logLevels = ["trace", "debug", "info", "warn", "error"]
    let tailscaleModes = ["off", "serve", "funnel"]

    /// Whether settings have been loaded from the backend at least once.
    @Published private(set) var isLoaded = false

    // MARK: - Private state

    let client = MoltisClient()
    /// Raw config dictionary for round-tripping. Modified in-place by section
    /// save methods and sent back to Rust as the full config JSON.
    var rawConfig: [String: Any] = [:]

    // MARK: - Load

    /// Loads all settings from the Rust backend (config file + identity files).
    func load() {
        do {
            let result = try client.getConfig()
            rawConfig = result.config
            environmentConfigDir = result.configDir
            environmentDataDir = result.dataDir
            populateFromConfig(result.config)
        } catch {
            logSettingsError("load config", error)
        }

        do {
            let identity = try client.getIdentity()
            identityName = identity.name
            identityEmoji = identity.emoji ?? ""
            identityTheme = identity.theme ?? ""
            identityUserName = identity.userName ?? ""
        } catch {
            logSettingsError("load identity", error)
        }

        do {
            identitySoul = try client.getSoul() ?? ""
        } catch {
            logSettingsError("load soul", error)
        }

        isLoaded = true
    }

    // MARK: - Section saves

    func saveIdentity() {
        let name = identityName.isEmpty ? nil : identityName
        let emoji = identityEmoji.isEmpty ? nil : identityEmoji
        let theme = identityTheme.isEmpty ? nil : identityTheme
        do {
            try client.saveIdentity(name: name, emoji: emoji, theme: theme)
        } catch {
            logSettingsError("save identity", error)
        }
    }

    func saveUserProfile() {
        let name = identityUserName.isEmpty ? nil : identityUserName
        do {
            try client.saveUserProfile(name: name)
        } catch {
            logSettingsError("save user profile", error)
        }
    }

    func saveSoul() {
        let text = identitySoul.isEmpty ? nil : identitySoul
        do {
            try client.saveSoul(text)
        } catch {
            logSettingsError("save soul", error)
        }
    }

    func saveHeartbeat() {
        setConfigValue(heartbeatEnabled, at: ["heartbeat", "enabled"])
        let durationStr = "\(heartbeatIntervalMinutes)m"
        setConfigValue(durationStr, at: ["heartbeat", "every"])
        persistConfig("heartbeat")
    }

    func saveMemory() {
        setConfigValue(!memoryEnabled, at: ["memory", "disable_rag"])
        persistConfig("memory")
    }

    func saveSecurity() {
        setConfigValue(!requirePassword, at: ["auth", "disabled"])
        persistConfig("security")
    }

    func saveTailscale() {
        setConfigValue(tailscaleMode, at: ["tailscale", "mode"])
        persistConfig("tailscale")
    }

    func saveMonitoring() {
        setConfigValue(metricsEnabled, at: ["metrics", "enabled"])
        setConfigValue(metricsEnabled, at: ["metrics", "prometheus_endpoint"])
        persistConfig("monitoring")
    }

    func saveGraphql() {
        setConfigValue(graphqlEnabled, at: ["graphql", "enabled"])
        persistConfig("graphql")
    }

    func saveSandbox() {
        setConfigValue(sandboxBackend, at: ["tools", "exec", "sandbox", "backend"])
        let image: Any = sandboxImage.isEmpty ? NSNull() : sandboxImage
        setConfigValue(image, at: ["tools", "exec", "sandbox", "image"])
        persistConfig("sandbox")
    }

    func saveChannels() {
        // Convert ChannelItem array back to config shape.
        // Each channel type is a HashMap<String, Value> keyed by channel name.
        var telegram: [String: Any] = [:]
        var discord: [String: Any] = [:]
        // (Other channel types can be added as needed)

        for channel in channels {
            var entry: [String: Any] = [
                "enabled": channel.enabled
            ]
            if !channel.botToken.isEmpty {
                entry["bot_token"] = channel.botToken
            }
            switch channel.channelType {
            case "telegram":
                telegram[channel.name.isEmpty ? "default" : channel.name] = entry
            case "discord":
                discord[channel.name.isEmpty ? "default" : channel.name] = entry
            default:
                telegram[channel.name.isEmpty ? "default" : channel.name] = entry
            }
        }

        setConfigValue(telegram, at: ["channels", "telegram"])
        setConfigValue(discord, at: ["channels", "discord"])
        persistConfig("channels")
    }

    func saveHooks() {
        let hookEntries: [[String: Any]] = hooks.map { hook in
            var entry: [String: Any] = [
                "name": hook.name,
                "command": hook.command,
                "events": [hook.event]
            ]
            if !hook.enabled {
                entry["timeout"] = 0
            }
            return entry
        }
        setConfigValue(["hooks": hookEntries], at: ["hooks"])
        persistConfig("hooks")
    }

    func saveMcp() {
        var servers: [String: Any] = [:]
        for item in mcpServers {
            let key = item.name.isEmpty ? "unnamed" : item.name
            var entry: [String: Any] = [
                "enabled": item.enabled,
                "transport": item.transport.rawValue
            ]
            if item.transport == .stdio, !item.command.isEmpty {
                entry["command"] = item.command
            }
            if item.transport == .sse, !item.url.isEmpty {
                entry["url"] = item.url
            }
            servers[key] = entry
        }
        setConfigValue(servers, at: ["mcp", "servers"])
        persistConfig("mcp")
    }

    func saveSkills() {
        let paths: [String] = skillPacks.map { $0.source }
        let autoLoad: [String] = skillPacks.filter { $0.enabled }.map { $0.source }
        setConfigValue(paths, at: ["skills", "search_paths"])
        setConfigValue(autoLoad, at: ["skills", "auto_load"])
        persistConfig("skills")
    }

    func saveVoice() {
        setConfigValue(voiceEnabled, at: ["voice", "tts", "enabled"])
        if !voiceProvider.isEmpty, voiceProvider != "none" {
            setConfigValue(voiceProvider, at: ["voice", "tts", "provider"])
        }
        persistConfig("voice")
    }
}

// MARK: - Config population and persistence

extension AppSettings {
    func populateFromConfig(_ config: [String: Any]) {
        populateToggles(from: config)
        populateCollections(from: config)
    }

    private func populateToggles(from config: [String: Any]) {
        if let heartbeat = config["heartbeat"] as? [String: Any] {
            heartbeatEnabled = heartbeat["enabled"] as? Bool ?? true
            if let every = heartbeat["every"] as? String {
                heartbeatIntervalMinutes = parseMinutes(from: every) ?? 5
            }
        }
        if let memory = config["memory"] as? [String: Any] {
            memoryEnabled = !(memory["disable_rag"] as? Bool ?? false)
        }
        if let auth = config["auth"] as? [String: Any] {
            requirePassword = !(auth["disabled"] as? Bool ?? false)
        }
        if let tailscale = config["tailscale"] as? [String: Any] {
            tailscaleMode = tailscale["mode"] as? String ?? "off"
            tailscaleEnabled = tailscaleMode != "off"
        }
        if let metrics = config["metrics"] as? [String: Any] {
            metricsEnabled = metrics["enabled"] as? Bool ?? true
            monitoringEnabled = metricsEnabled
        }
        if let graphql = config["graphql"] as? [String: Any] {
            graphqlEnabled = graphql["enabled"] as? Bool ?? false
        }
        if let tools = config["tools"] as? [String: Any],
           let exec = tools["exec"] as? [String: Any],
           let sandbox = exec["sandbox"] as? [String: Any] {
            sandboxBackend = sandbox["backend"] as? String ?? "auto"
            sandboxImage = sandbox["image"] as? String ?? "moltis/sandbox:latest"
        }
        if let voice = config["voice"] as? [String: Any],
           let tts = voice["tts"] as? [String: Any] {
            voiceEnabled = tts["enabled"] as? Bool ?? false
            voiceProvider = tts["provider"] as? String ?? "none"
        }
    }

    private func populateCollections(from config: [String: Any]) {
        populateChannels(from: config)
        populateHooksAndServers(from: config)
    }

    private func populateChannels(from config: [String: Any]) {
        channels = []
        guard let channelsConfig = config["channels"] as? [String: Any] else { return }
        for channelType in ["telegram", "discord", "whatsapp", "msteams"] {
            guard let typeMap = channelsConfig[channelType] as? [String: Any] else { continue }
            for (name, value) in typeMap {
                guard let entry = value as? [String: Any] else { continue }
                channels.append(ChannelItem(
                    name: name,
                    channelType: channelType,
                    botToken: entry["bot_token"] as? String ?? "",
                    enabled: entry["enabled"] as? Bool ?? true
                ))
            }
        }
    }

    private func populateHooksAndServers(from config: [String: Any]) {
        hooks = []
        if let hooksConfig = config["hooks"] as? [String: Any],
           let hooksList = hooksConfig["hooks"] as? [[String: Any]] {
            for entry in hooksList {
                let events = entry["events"] as? [String] ?? []
                hooks.append(HookItem(
                    name: entry["name"] as? String ?? "",
                    event: events.first ?? "on_message",
                    command: entry["command"] as? String ?? "",
                    enabled: true
                ))
            }
        }
        mcpServers = []
        if let mcp = config["mcp"] as? [String: Any],
           let servers = mcp["servers"] as? [String: Any] {
            for (name, value) in servers {
                guard let entry = value as? [String: Any] else { continue }
                let transportStr = entry["transport"] as? String ?? "stdio"
                let transport: McpTransport = transportStr == "sse" ? .sse : .stdio
                mcpServers.append(McpServerItem(
                    name: name,
                    transport: transport,
                    command: entry["command"] as? String ?? "",
                    url: entry["url"] as? String ?? "",
                    enabled: entry["enabled"] as? Bool ?? true
                ))
            }
        }
        skillPacks = []
        if let skills = config["skills"] as? [String: Any] {
            let searchPaths = skills["search_paths"] as? [String] ?? []
            let autoLoad = Set(skills["auto_load"] as? [String] ?? [])
            for path in searchPaths {
                skillPacks.append(SkillPackItem(
                    source: path,
                    repoName: URL(fileURLWithPath: path).lastPathComponent,
                    enabled: autoLoad.contains(path)
                ))
            }
        }
    }

    /// Sets a value in the raw config dictionary at the given key path.
    func setConfigValue(_ value: Any, at keyPath: [String]) {
        guard !keyPath.isEmpty else { return }
        if keyPath.count == 1 {
            rawConfig[keyPath[0]] = value
            return
        }
        // Walk into nested dictionaries, creating them as needed.
        var current = rawConfig
        var parents: [([String: Any], String)] = []

        for key in keyPath.dropLast() {
            parents.append((current, key))
            current = current[key] as? [String: Any] ?? [:]
        }
        current[keyPath.last!] = value // swiftlint:disable:this force_unwrapping

        // Walk back up rebuilding parent dictionaries.
        for (parent, key) in parents.reversed() {
            var rebuilt = parent
            rebuilt[key] = current
            current = rebuilt
        }
        rawConfig = current
    }

    /// Persists the current rawConfig to the backend.
    func persistConfig(_ context: String) {
        do {
            try client.saveConfig(rawConfig)
        } catch {
            logSettingsError("save \(context)", error)
        }
    }

    /// Parses a duration string like "30m" or "1h" into minutes.
    func parseMinutes(from duration: String) -> Int? {
        let trimmed = duration.trimmingCharacters(in: .whitespaces)
        if trimmed.hasSuffix("m"), let value = Int(trimmed.dropLast()) {
            return value
        }
        if trimmed.hasSuffix("h"), let value = Int(trimmed.dropLast()) {
            return value * 60
        }
        if trimmed.hasSuffix("s"), let value = Int(trimmed.dropLast()) {
            return max(1, value / 60)
        }
        return Int(trimmed)
    }

    func logSettingsError(_ context: String, _ error: Error) {
        print("[AppSettings] Failed to \(context): \(error.localizedDescription)")
    }
}
