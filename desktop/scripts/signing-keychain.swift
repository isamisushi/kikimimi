import Foundation
import Security

// Secrets travel only through a private stdin/stdout pipe and the login Keychain.
let service = "dev.kikimimi.desktop.release"
let account = "signing-material-v1"

func checked(_ status: OSStatus, _ operation: String) throws {
    if status != errSecSuccess {
        throw NSError(domain: operation, code: Int(status))
    }
}

func query(_ name: String = account) -> [String: Any] {
    [kSecClass as String: kSecClassGenericPassword,
     kSecAttrService as String: service,
     kSecAttrAccount as String: name]
}

func store(_ data: Data, name: String = account) throws {
    let status = SecItemUpdate(query(name) as CFDictionary,
                               [kSecValueData as String: data] as CFDictionary)
    if status == errSecItemNotFound {
        var item = query(name)
        item[kSecValueData as String] = data
        var trusted: SecTrustedApplication?
        try checked(SecTrustedApplicationCreateFromPath(CommandLine.arguments[0], &trusted), "Trusted helper")
        var securityTool: SecTrustedApplication?
        try checked(SecTrustedApplicationCreateFromPath("/usr/bin/security", &securityTool), "Trusted Keychain tool")
        var access: SecAccess?
        try checked(SecAccessCreate("kikimimi desktop release" as CFString,
                                    [trusted!, securityTool!] as CFArray, &access), "Keychain access")
        item[kSecAttrAccess as String] = access!
        try checked(SecItemAdd(item as CFDictionary, nil), "Keychain store")
    } else {
        try checked(status, "Keychain update")
    }
}

func load(_ name: String = account) throws -> Data {
    var item = query(name)
    item[kSecReturnData as String] = true
    item[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    try checked(SecItemCopyMatching(item as CFDictionary, &result), "Keychain read")
    guard let data = result as? Data else {
        throw NSError(domain: "Keychain data", code: 1)
    }
    return data
}

func importCertificate(_ data: Data) throws {
    guard let values = try JSONSerialization.jsonObject(with: data) as? [String: String],
          let encoded = values["APPLE_CERTIFICATE"],
          let password = values["APPLE_CERTIFICATE_PASSWORD"],
          let p12 = Data(base64Encoded: encoded, options: .ignoreUnknownCharacters) else {
        throw NSError(domain: "Certificate input", code: 1)
    }
    var keychain: SecKeychain?
    try checked(SecKeychainCopyDefault(&keychain), "Default Keychain")
    var codesign: SecTrustedApplication?
    try checked(SecTrustedApplicationCreateFromPath("/usr/bin/codesign", &codesign), "Trusted signer")
    var access: SecAccess?
    try checked(SecAccessCreate("kikimimi Developer ID signing" as CFString,
                                [codesign!] as CFArray, &access), "Signing access")
    let options: [String: Any] = [
        kSecImportExportPassphrase as String: password,
        kSecImportExportKeychain as String: keychain!,
        kSecImportExportAccess as String: access!
    ]
    var items: CFArray?
    let status = SecPKCS12Import(p12 as CFData, options as CFDictionary, &items)
    if status != errSecDuplicateItem {
        try checked(status, "Developer ID import")
    }
}

do {
    switch CommandLine.arguments.dropFirst().first {
    case "store":
        let data = FileHandle.standardInput.readDataToEndOfFile()
        let required = ["APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_ID", "APPLE_PASSWORD", "TAURI_SIGNING_PRIVATE_KEY", "KIKIMIMI_UPDATER_PUBLIC_KEY"]
        guard let values = try JSONSerialization.jsonObject(with: data) as? [String: String],
              required.allSatisfy({ !(values[$0] ?? "").isEmpty }) else {
            throw NSError(domain: "Signing input", code: 1)
        }
        try store(data)
        print("Signing settings stored in login Keychain")
    case "import-certificate":
        try importCertificate(FileHandle.standardInput.readDataToEndOfFile())
        print("Developer ID certificate imported into login Keychain")
    case "self-test":
        let name = "self-test-" + UUID().uuidString
        let data = Data(UUID().uuidString.utf8)
        defer { SecItemDelete(query(name) as CFDictionary) }
        try store(data, name: name)
        guard try load(name) == data else {
            throw NSError(domain: "Keychain round trip", code: 1)
        }
        print("PASS: Keychain store/read/delete with isolated test item")
    default:
        throw NSError(domain: "Expected store, import-certificate, or self-test", code: 1)
    }
} catch {
    // Do not serialize input, Keychain values, or system error objects.
    let failure = error as NSError
    FileHandle.standardError.write(Data("Signing Keychain operation failed (\(failure.domain), \(failure.code))\n".utf8))
    exit(1)
}
