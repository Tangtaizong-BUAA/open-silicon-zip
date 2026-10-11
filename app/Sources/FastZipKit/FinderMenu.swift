import Foundation

/// Platform-neutral description of a Finder context-menu entry. The Finder Sync
/// extension turns these into `NSMenuItem`s; keeping the logic here makes it testable.
public struct MenuSpec: Equatable, Sendable {
    public var title: String
    public var link: DeepLink?
    public var children: [MenuSpec]
    public var isSeparator: Bool

    public init(title: String, link: DeepLink? = nil, children: [MenuSpec] = [], isSeparator: Bool = false) {
        self.title = title
        self.link = link
        self.children = children
        self.isSeparator = isSeparator
    }

    public static let separator = MenuSpec(title: "", isSeparator: true)
}

/// Composes the menus of FR-502 (compression), FR-504 (extraction) and FR-510 (scope controls),
/// laid out as 05 section 8: primary one-click section, presets section, L10n.t("More") submenu.
public enum FinderMenuBuilder {

    public static var rootTitle: String { "Open Silicon Zip" }

    public static func build(selection: [URL], config: FinderConfig, now: Date = Date()) -> [MenuSpec] {
        let authorised = selection.filter {
            config.isAuthorised(path: $0.path, isExternalVolume: $0.path.hasPrefix("/Volumes/"))
        }
        guard !authorised.isEmpty, authorised.count == selection.count else { return [] }

        let archives = authorised.filter { config.isArchive(path: $0.path) }
        let others = authorised.filter { !config.isArchive(path: $0.path) }
        let paths = authorised.map(\.path)

        var primary: [MenuSpec] = []
        var presets: [MenuSpec] = []
        var more: [MenuSpec] = []

        // Compression
        if !others.isEmpty || (!archives.isEmpty && config.showOnArchiveFiles) {
            let name = baseName(for: authorised)
            let multiple = authorised.count > 1
            primary.append(MenuSpec(title: L10n.t("Compress with FastZip..."), link: .new(paths: paths, preset: nil)))
            for format in [ArchiveFormat.zip, .sevenZ] {
                let ext = format.fileExtension
                primary.append(MenuSpec(title: L10n.f("Compress to \"%@.%@\"", name, ext),
                                        link: .compress(paths: paths, format: format, deleteSources: false,
                                                        each: false, preset: nil, rule: nil)))
                more.append(MenuSpec(title: L10n.f("Compress to \"%@.%@\" and delete source", name, ext),
                                     link: .compress(paths: paths, format: format, deleteSources: true,
                                                     each: false, preset: nil, rule: nil)))
                if multiple {
                    more.append(MenuSpec(title: L10n.f("Compress each to .%@", ext),
                                         link: .compress(paths: paths, format: format, deleteSources: false,
                                                         each: true, preset: nil, rule: nil)))
                    more.append(MenuSpec(title: L10n.f("Compress each to .%@ and delete sources", ext),
                                         link: .compress(paths: paths, format: format, deleteSources: true,
                                                         each: true, preset: nil, rule: nil)))
                }
            }
            if config.customFilenameRule != "${NAME}" && !config.customFilenameRule.isEmpty {
                let expanded = FilenameRule.expand(config.customFilenameRule, name: name, date: now)
                primary.append(MenuSpec(title: L10n.f("Compress to \"%@.zip\"", expanded),
                                        link: .compress(paths: paths, format: .zip, deleteSources: false, each: false,
                                                        preset: nil, rule: config.customFilenameRule)))
            }
            // Presets (FR-1102): default preset first.
            let ordered = config.presetNames.sorted { a, b in
                (a == config.defaultPresetName ? 0 : 1, a) < (b == config.defaultPresetName ? 0 : 1, b)
            }
            for p in ordered {
                presets.append(MenuSpec(title: L10n.f("Compress with preset \"%@\"", p),
                                        link: .compress(paths: paths, format: .zip, deleteSources: false, each: false,
                                                        preset: p, rule: nil)))
            }
        }

        // Extraction (only when every item is an archive)
        if !archives.isEmpty && others.isEmpty {
            let apaths = archives.map(\.path)
            let single = archives.count == 1
            let base = SmartExtract.archiveBaseName(archives[0].lastPathComponent)
            primary.append(MenuSpec(title: single ? L10n.f("Extract to \"%@/\"", base) : L10n.t("Extract Each to Separate Folders"),
                                    link: .extract(paths: apaths, mode: .named, deleteArchive: false)))
            primary.append(MenuSpec(title: L10n.t("Extract Here"), link: .extract(paths: apaths, mode: .here, deleteArchive: false)))
            primary.append(MenuSpec(title: L10n.t("Extract to..."), link: .extract(paths: apaths, mode: .other, deleteArchive: false)))
            primary.append(MenuSpec(title: L10n.t("Open with FastZip"), link: .open(paths: apaths)))
            primary.append(MenuSpec(title: L10n.t("Test Archive"), link: .test(paths: apaths)))
            more.append(MenuSpec(title: L10n.t("Extract Here (Smart) and delete archive"),
                                 link: .extract(paths: apaths, mode: .smart, deleteArchive: true)))
            more.append(MenuSpec(title: L10n.t("Extract Here and delete archive"),
                                 link: .extract(paths: apaths, mode: .here, deleteArchive: true)))
            more.append(MenuSpec(title: single ? L10n.f("Extract to \"%@/\" and delete archive", base) : L10n.t("Extract Each and delete archives"),
                                 link: .extract(paths: apaths, mode: .named, deleteArchive: true)))
        }

        if config.showChecksumSubmenu {
            more.append(MenuSpec(title: L10n.t("CRC / Hash..."), link: .checksum(paths: paths)))
        }

        var items = primary
        if !presets.isEmpty { items += [.separator] + presets }
        if !more.isEmpty { items += [.separator, MenuSpec(title: L10n.t("More"), children: more)] }
        guard !items.isEmpty else { return [] }

        return config.cascaded ? [MenuSpec(title: rootTitle, children: items)] : items
    }

    /// Name used in "Compress to <name>.zip": the item itself, or the parent folder for multi-selection.
    static func baseName(for urls: [URL]) -> String {
        guard let first = urls.first else { return "Archive" }
        if urls.count == 1 { return first.lastPathComponent }
        return first.deletingLastPathComponent().lastPathComponent
    }

    /// Whether the action needs a window (otherwise it can be handed off silently through the job inbox).
    public static func needsUI(_ link: DeepLink) -> Bool {
        switch link {
        case .new, .open, .checksum: return true
        case .extract(_, let mode, _): return mode == .other || mode == .ask
        case .compress, .test: return false
        }
    }
}
