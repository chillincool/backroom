import BackroomCore
import SwiftUI

struct ContentView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            switch model.phase {
            case .online:
                MainView()
            case .loggedOut, .loggingIn, .failed:
                LoginView()
            }
        }
        .safeAreaInset(edge: .bottom) {
            if let notice = model.notice {
                NoticeBar(text: notice) { model.notice = nil }
            }
        }
    }
}

struct NoticeBar: View {
    let text: String
    let dismiss: () -> Void

    var body: some View {
        HStack {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.yellow)
            Text(text).textSelection(.enabled)
            Spacer()
            Button("Dismiss", action: dismiss)
        }
        .padding(10)
        .background(.bar)
    }
}

struct LoginView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 48))
                .foregroundStyle(.secondary)
            Text("Backroom").font(.largeTitle.bold())
            Text(model.settings.serverURL)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)

            if case .failed(let message) = model.phase {
                Text("Couldn't connect: \(message)")
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
                Button("Try Again") { Task { await model.start() } }
            }

            Button {
                Task { await model.logIn() }
            } label: {
                if model.phase == .loggingIn {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Log In")
                }
            }
            .keyboardShortcut(.defaultAction)
            .controlSize(.large)
            .disabled(model.phase == .loggingIn)

            if model.phase == .loggingIn {
                Text("Finish logging in in your browser.")
                    .foregroundStyle(.secondary)
                Button("Cancel") { model.cancelLogin() }
            }

            SettingsLink { Text("Server Settings…") }
                .buttonStyle(.link)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
