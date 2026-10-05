// Temporary, Founder-authorized negative control for PR #126.
// Reverted after the required compiler catalog check rejects this copy.
import SwiftUI

struct L10NGateNegativeControl: View {
  var body: some View {
    Text("PR 126 negative control: catalog synchronization required")
  }
}
