import Foundation

/// A URL, Services message or inbox file carries intent, not caller authority.
public enum ExternalRequestPolicy {
    public static func validate(_ paths: [String], config: FinderConfig) throws {
        guard !paths.isEmpty, paths.count <= 1024 else { throw denied() }
        for path in paths {
            guard path.hasPrefix("/"), !path.contains("\0"),
                  !path.split(separator: "/").contains(".."),
                  path.utf8.count <= 4096,
                  config.isAuthorised(path: path) else { throw denied() }
        }
    }

    /// A mixed-directory request must get an output folder from the user;
    /// reordering attacker-controlled inputs cannot change that folder.
    public static func commonSourceDirectory(_ paths: [String]) -> URL? {
        let parents = Set(paths.map {
            URL(fileURLWithPath: $0).deletingLastPathComponent()
                .resolvingSymlinksInPath().standardizedFileURL.path
        })
        guard parents.count == 1, let parent = parents.first else { return nil }
        return URL(fileURLWithPath: parent, isDirectory: true)
    }

    public static func compressionSettings(_ settings: CreateSettings, deleteSources: Bool) -> CreateSettings {
        var result = settings
        // The URL's explicit false is a veto, including when a preset says true.
        result.options.deleteSources = deleteSources
        return result
    }

    private static func denied() -> FZError {
        FZError(code: .unsafePath, message: L10n.t("An external request includes a path outside the configured accessible folders."))
    }
}

public extension DeepLink {
    var paths: [String] {
        switch self {
        case .open(let paths), .new(let paths, _), .extract(let paths, _, _),
             .compress(let paths, _, _, _, _, _), .test(let paths), .checksum(let paths): return paths
        }
    }
}
