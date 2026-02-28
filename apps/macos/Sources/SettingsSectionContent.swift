import SwiftUI

/// Returns raw form controls for a given settings section.
/// Designed to be placed inside a `Form` `Section`.
struct SettingsSectionContent: View {
    let section: SettingsSection
    @ObservedObject var settings: AppSettings
    @ObservedObject var providerStore: ProviderStore
    var logStore: LogStore?

    var body: some View {
        switch section {
        case .identity: identityPane
        case .environment: environmentPane
        case .memory: memoryPane
        case .notifications: notificationsPane
        case .crons: cronsPane
        case .heartbeat: heartbeatPane
        case .security: securityPane
        case .tailscale: tailscalePane
        case .channels: channelsPane
        case .hooks: hooksPane
        case .llms: llmsPane
        case .mcp: mcpPane
        case .skills: skillsPane
        case .voice: voicePane
        case .sandboxes: sandboxesPane
        case .networkAudit: networkAuditPane
        case .monitoring: monitoringPane
        case .logs: logsPane
        case .graphql: graphqlPane
        case .httpd: httpdPane
        case .configuration: configurationPane
        }
    }
}

// MARK: - General

private extension SettingsSectionContent {
    var identityPane: some View {
        Group {
            Section("Agent") {
                TextField("Name", text: $settings.identityName, prompt: Text("e.g. Rex"))
                    .onSubmit { settings.saveIdentity() }
                TextField("Emoji", text: $settings.identityEmoji)
                    .onSubmit { settings.saveIdentity() }
                TextField("Theme", text: $settings.identityTheme, prompt: Text("e.g. wise owl, chill fox"))
                    .onSubmit { settings.saveIdentity() }
            }
            Section("User") {
                TextField("Your name", text: $settings.identityUserName, prompt: Text("e.g. Alice"))
                    .onSubmit { settings.saveUserProfile() }
            }
            editorRow("Soul", text: $settings.identitySoul)
                .onChange(of: settings.identitySoul) { settings.saveSoul() }
        }
    }

    var memoryPane: some View {
        Group {
            Toggle("Enable memory", isOn: $settings.memoryEnabled)
                .onChange(of: settings.memoryEnabled) { settings.saveMemory() }
            Picker("Memory mode", selection: $settings.memoryMode) {
                ForEach(settings.memoryModes, id: \.self) { mode in
                    Text(mode.capitalized).tag(mode)
                }
            }
        }
    }

    var notificationsPane: some View {
        Group {
            Toggle("Enable notifications", isOn: $settings.notificationsEnabled)
            Toggle("Play sounds", isOn: $settings.notificationsSoundEnabled)
        }
    }

    var cronsPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            if settings.cronJobs.isEmpty {
                SettingsEmptyState(
                    icon: "clock.arrow.circlepath",
                    title: "No Cron Jobs",
                    subtitle: "Scheduled tasks require the gateway to be running"
                )
            } else {
                ForEach($settings.cronJobs) { $item in
                    DisclosureGroup {
                        cronJobFields(item: $item)
                    } label: {
                        cronJobLabel(item: $item)
                    }
                }
            }
            Button {
                settings.cronJobs.append(CronJobItem())
            } label: {
                Label("Add Cron Job", systemImage: "plus")
            }
        }
    }

    var heartbeatPane: some View {
        Group {
            Toggle("Enable heartbeat", isOn: $settings.heartbeatEnabled)
                .onChange(of: settings.heartbeatEnabled) { settings.saveHeartbeat() }
            Stepper(
                String(
                    format: NSLocalizedString(
                        "Interval: %d min",
                        comment: "Heartbeat interval in minutes"
                    ),
                    settings.heartbeatIntervalMinutes
                ),
                value: $settings.heartbeatIntervalMinutes,
                in: 1 ... 120
            )
            .onChange(of: settings.heartbeatIntervalMinutes) { settings.saveHeartbeat() }
        }
    }
}

// MARK: - Security

private extension SettingsSectionContent {
    var securityPane: some View {
        Group {
            Toggle("Require password login", isOn: $settings.requirePassword)
                .onChange(of: settings.requirePassword) { settings.saveSecurity() }
            Toggle("Enable passkeys", isOn: $settings.passkeysEnabled)
        }
    }

    var tailscalePane: some View {
        Group {
            Picker("Tailscale mode", selection: $settings.tailscaleMode) {
                ForEach(settings.tailscaleModes, id: \.self) { mode in
                    Text(mode.capitalized).tag(mode)
                }
            }
            .onChange(of: settings.tailscaleMode) {
                settings.tailscaleEnabled = settings.tailscaleMode != "off"
                settings.saveTailscale()
            }
        }
    }
}

// MARK: - Integrations

private extension SettingsSectionContent {
    var channelsPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            if settings.channels.isEmpty {
                SettingsEmptyState(
                    icon: "point.3.connected.trianglepath.dotted",
                    title: "No Channels",
                    subtitle: "Connect messaging platforms like Telegram or Slack"
                )
            } else {
                ForEach($settings.channels) { $item in
                    DisclosureGroup {
                        channelFields(item: $item)
                    } label: {
                        channelLabel(item: $item)
                    }
                }
            }
            Button {
                settings.channels.append(ChannelItem())
                settings.saveChannels()
            } label: {
                Label("Add Channel", systemImage: "plus")
            }
        }
    }

    var hooksPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            if settings.hooks.isEmpty {
                SettingsEmptyState(
                    icon: "wrench.and.screwdriver",
                    title: "No Hooks",
                    subtitle: "Run commands in response to events"
                )
            } else {
                ForEach($settings.hooks) { $item in
                    DisclosureGroup {
                        hookFields(item: $item)
                    } label: {
                        hookLabel(item: $item)
                    }
                }
            }
            Button {
                settings.hooks.append(HookItem())
                settings.saveHooks()
            } label: {
                Label("Add Hook", systemImage: "plus")
            }
        }
    }

    var llmsPane: some View {
        ProviderGridPane(providerStore: providerStore)
    }

    var mcpPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            if settings.mcpServers.isEmpty {
                SettingsEmptyState(
                    icon: "link",
                    title: "No MCP Servers",
                    subtitle: "Connect external tools via Model Context Protocol"
                )
            } else {
                ForEach($settings.mcpServers) { $item in
                    DisclosureGroup {
                        mcpFields(item: $item)
                    } label: {
                        mcpLabel(item: $item)
                    }
                }
            }
            Button {
                settings.mcpServers.append(McpServerItem())
                settings.saveMcp()
            } label: {
                Label("Add MCP Server", systemImage: "plus")
            }
        }
    }

    var skillsPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            if settings.skillPacks.isEmpty {
                SettingsEmptyState(
                    icon: "sparkles",
                    title: "No Skill Packs",
                    subtitle: "Install skill packs to extend capabilities"
                )
            } else {
                ForEach($settings.skillPacks) { $item in
                    DisclosureGroup {
                        skillFields(item: $item)
                    } label: {
                        skillLabel(item: $item)
                    }
                }
            }
            Button {
                settings.skillPacks.append(SkillPackItem())
                settings.saveSkills()
            } label: {
                Label("Add Skill Pack", systemImage: "plus")
            }
        }
    }

    var voicePane: some View {
        VoiceProviderGridPane(
            providerStore: providerStore,
            settings: settings
        )
    }
}

// MARK: - Systems

private extension SettingsSectionContent {
    var sandboxesPane: some View {
        Group {
            Picker("Backend", selection: $settings.sandboxBackend) {
                ForEach(settings.sandboxBackends, id: \.self) { backend in
                    Text(backend.capitalized).tag(backend)
                }
            }
            .onChange(of: settings.sandboxBackend) { settings.saveSandbox() }
            TextField("Default image", text: $settings.sandboxImage)
                .onSubmit { settings.saveSandbox() }
        }
    }

    var networkAuditPane: some View {
        SettingsEmptyState(
            icon: "network.badge.shield.half.filled",
            title: "Network Audit",
            subtitle: "Select this section to view the full network audit log"
        )
    }

    var monitoringPane: some View {
        Group {
            Toggle("Enable monitoring", isOn: $settings.monitoringEnabled)
            Toggle("Enable metrics", isOn: $settings.metricsEnabled)
                .onChange(of: settings.metricsEnabled) { settings.saveMonitoring() }
            Toggle("Enable tracing", isOn: $settings.tracingEnabled)
        }
    }

    @ViewBuilder
    var logsPane: some View {
        if let logStore {
            LogsPane(logStore: logStore)
        } else {
            SettingsEmptyState(
                icon: "doc.plaintext",
                title: "Logs Unavailable",
                subtitle: "Log store not connected"
            )
        }
    }

    var graphqlPane: some View {
        Group {
            Toggle("Enable GraphQL", isOn: $settings.graphqlEnabled)
                .onChange(of: settings.graphqlEnabled) { settings.saveGraphql() }
        }
    }

    var httpdPane: some View {
        HttpdPane(settings: settings)
    }

    var configurationPane: some View {
        ConfigurationPane(settings: settings)
    }
}

// MARK: - Helpers

extension SettingsSectionContent {
    /// Full-width editor row with label above.
    func editorRow(
        _ title: String,
        text: Binding<String>,
        minHeight: CGFloat = 160
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .foregroundStyle(.secondary)
            MoltisEditorField(text: text, minHeight: minHeight)
        }
    }

    func deleteButton(action: @escaping () -> Void) -> some View {
        Button(role: .destructive, action: action) {
            Image(systemName: "trash")
                .foregroundStyle(.red)
        }
        .buttonStyle(.borderless)
    }
}
