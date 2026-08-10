import Foundation
import XCTest
@testable import Codescribe

private final class ActivationPingTransportSpy: ActivationPingTransport {
    private(set) var requests: [URLRequest] = []
    var statusCode = 202

    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        requests.append(request)
        let response = HTTPURLResponse(
            url: request.url!,
            statusCode: statusCode,
            httpVersion: "HTTP/1.1",
            headerFields: nil
        )!
        return (Data(), response)
    }
}

@MainActor
final class ActivationPingTests: XCTestCase {
    private var defaults: UserDefaults!
    private var suiteName: String!

    override func setUp() {
        super.setUp()
        suiteName = "ActivationPingTests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)
        defaults.removePersistentDomain(forName: suiteName)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suiteName)
        defaults = nil
        suiteName = nil
        super.tearDown()
    }

    func testOptInDefaultsOffAndProducesZeroNetworkRequests() async {
        let transport = ActivationPingTransportSpy()
        let ping = makePing(transport: transport)

        XCTAssertFalse(defaults.bool(forKey: ActivationPing.optInDefaultsKey))
        let outcome = await ping.recordFirstSuccessfulDictation()
        XCTAssertEqual(outcome, .notConsented)
        XCTAssertTrue(transport.requests.isEmpty)
    }

    func testOptInSendsExactlyOneContentFreeEventAndPersistsReceipt() async throws {
        defaults.set(true, forKey: ActivationPing.optInDefaultsKey)
        let transport = ActivationPingTransportSpy()
        let ping = makePing(transport: transport)

        let firstOutcome = await ping.recordFirstSuccessfulDictation()
        let secondOutcome = await ping.recordFirstSuccessfulDictation()
        XCTAssertEqual(firstOutcome, .sent)
        XCTAssertEqual(secondOutcome, .alreadySent)
        XCTAssertEqual(transport.requests.count, 1)
        XCTAssertTrue(defaults.bool(forKey: ActivationPing.sentDefaultsKey))

        let request = try XCTUnwrap(transport.requests.first)
        XCTAssertEqual(request.url?.absoluteString, "https://plausible.test/api/event")
        XCTAssertEqual(request.httpMethod, "POST")
        let body = try XCTUnwrap(request.httpBody)
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: body) as? [String: Any]
        )
        XCTAssertEqual(object["name"] as? String, "first_successful_dictation")
        XCTAssertEqual(object["domain"] as? String, "dev.codescribe.test")
        XCTAssertEqual(object["url"] as? String, "https://dev.codescribe.test/app/activation")
        XCTAssertEqual(
            object["props"] as? [String: String],
            ["app_version": "9.9.9-test", "os": "macOS test"]
        )
        XCTAssertNil(object["text"])
        XCTAssertFalse(String(decoding: body, as: UTF8.self).contains("transcript"))
    }

    func testMissingB3DomainKeepsNetworkDisabledEvenAfterOptIn() async {
        defaults.set(true, forKey: ActivationPing.optInDefaultsKey)
        let transport = ActivationPingTransportSpy()
        let ping = ActivationPing(
            configuration: .production,
            defaults: defaults,
            transport: transport
        )

        let outcome = await ping.recordFirstSuccessfulDictation()
        XCTAssertEqual(outcome, .disabled)
        XCTAssertTrue(transport.requests.isEmpty)
    }

    private func makePing(transport: ActivationPingTransportSpy) -> ActivationPing {
        ActivationPing(
            configuration: ActivationPingConfiguration(
                endpoint: URL(string: "https://plausible.test/api/event")!,
                domain: "dev.codescribe.test"
            ),
            defaults: defaults,
            transport: transport,
            appVersion: { "9.9.9-test" },
            operatingSystem: { "macOS test" }
        )
    }
}
