import SwiftUI
import UIKit
import YeTrackerKit

/// One song as a list row: play/download for stored files, a source link
/// otherwise, title, expandable notes and the quality/length/date details (the
/// web table's phone layout). The era list draws the playing/highlight fill.
struct SongRow: View {
  let song: EraSong
  let palette: EraPalette
  let isCurrent: Bool
  let isPlaying: Bool
  let onPlay: @MainActor () -> Void
  let onDownload: @MainActor () -> Void
  let onOpenSource: @MainActor (URL) -> Void

  var body: some View {
    HStack(alignment: .top, spacing: 12) {
      primaryAction

      VStack(alignment: .leading, spacing: 6) {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
          Text(song.displayTitle)
            .font(.body.weight(isCurrent ? .semibold : .regular))
            .foregroundStyle(isCurrent ? palette.tint : .primary)
            .fixedSize(horizontal: false, vertical: true)
          if isCurrent {
            NowPlayingGlyph(isPlaying: isPlaying, color: palette.tint)
          }
        }
        if let notes = song.trimmedNotes {
          ExpandableText(text: notes, lineLimit: 3, tint: palette.tint)
            .font(.footnote)
            .foregroundStyle(.secondary)
        }
        if !details.isEmpty {
          Text(details)
            .font(.caption)
            .foregroundStyle(.tertiary)
        }
      }
      .frame(maxWidth: .infinity, alignment: .leading)

      if song.isPlayable {
        Button {
          onDownload()
        } label: {
          Image(systemName: "arrow.down.circle")
            .font(.title3)
            .foregroundStyle(.secondary)
            .frame(width: 32, height: 32)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Download \(song.displayTitle)")
      }
    }
    .contentShape(.rect)
    .onTapGesture {
      if song.isPlayable { onPlay() }
    }
    .contextMenu { menu }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(accessibilityText)
    .accessibilityHint(song.isPlayable ? "Plays the song." : song.unavailableReason)
    .accessibilityAddTraits(song.isPlayable ? .isButton : [])
    .accessibilityAction {
      if song.isPlayable {
        onPlay()
      } else if let url = song.sourceURL {
        onOpenSource(url)
      }
    }
    .accessibilityActions {
      if song.isPlayable {
        Button("Download original file") { onDownload() }
      }
      if let url = song.sourceURL {
        Button("Open source") { onOpenSource(url) }
      }
    }
  }

  /// Play for stored files, a source link otherwise, "—" when there is neither.
  @ViewBuilder
  private var primaryAction: some View {
    if song.isPlayable {
      Button {
        onPlay()
      } label: {
        Image(systemName: isCurrent && isPlaying ? "pause.fill" : "play.fill")
          .font(.subheadline.weight(.semibold))
          .foregroundStyle(palette.tint)
          .frame(width: 36, height: 36)
          .background(palette.controlFill, in: .circle)
          .contentTransition(.symbolEffect(.replace))
      }
      .buttonStyle(.plain)
    } else if let url = song.sourceURL {
      Button {
        onOpenSource(url)
      } label: {
        Image(systemName: "link")
          .font(.footnote.weight(.semibold))
          .foregroundStyle(palette.tint)
          .frame(width: 36, height: 36)
          .overlay(Circle().strokeBorder(palette.controlFill, lineWidth: 1.5))
          .contentShape(.circle)
      }
      .buttonStyle(.plain)
    } else {
      Text("—")
        .foregroundStyle(.tertiary)
        .frame(width: 36, height: 36)
    }
  }

  @ViewBuilder
  private var menu: some View {
    if song.isPlayable {
      Button("Play", systemImage: "play.fill") { onPlay() }
      Button("Download Original", systemImage: "arrow.down.circle") { onDownload() }
    }
    if let url = song.sourceURL {
      Button("Open Source", systemImage: "link") { onOpenSource(url) }
    }
    Button("Copy Title", systemImage: "doc.on.doc") {
      UIPasteboard.general.string = song.displayTitle
    }
    if !song.isPlayable {
      Text(song.unavailableReason)
    }
  }

  /// "CD Quality · Snippet - 1:05 · File 9/30/21 · Leaked 4/22/09".
  private var details: String {
    var parts: [String] = []
    if let quality = song.trimmedQuality { parts.append(quality) }
    if !song.lengthLabel.isEmpty { parts.append(song.lengthLabel) }
    if let file = Formatters.shortDate(song.fileDate) { parts.append("File \(file)") }
    if let leak = Formatters.shortDate(song.leakDate) { parts.append("Leaked \(leak)") }
    return parts.joined(separator: " · ")
  }

  private var accessibilityText: String {
    var parts = [song.displayTitle]
    if isCurrent { parts.append(isPlaying ? "now playing" : "paused") }
    if !details.isEmpty { parts.append(details) }
    if let notes = song.trimmedNotes { parts.append(notes) }
    return parts.joined(separator: ". ")
  }
}
