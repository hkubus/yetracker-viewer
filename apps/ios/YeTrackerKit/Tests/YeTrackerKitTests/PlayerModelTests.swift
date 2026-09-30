import Foundation
import Testing

@testable import YeTrackerKit

@Suite("PlayerModel")
@MainActor
struct PlayerModelTests {
  let http = FakeHTTPClient()
  let engine = FakePlaybackEngine()
  let nowPlaying = RecordingNowPlaying()

  func makePlayer(settings: AppSettings = makeSettings(), sleeper: @escaping Sleeper = foreverSleeper) -> PlayerModel {
    let player = PlayerModel(engine: engine, api: makeAPI(http), settings: settings, sleeper: sleeper)
    player.nowPlaying = nowPlaying
    return player
  }

  @Test func playStartsTheDefaultAACTranscode() throws {
    let player = makePlayer()
    let first = track(1, duration: 100)
    player.play(first, queue: [first, track(2, duration: 90)], queueID: "list")

    let source = try #require(engine.lastSource)
    #expect(source.url.absoluteString == "http://test.local/songs/1/stream?quality=128&format=aac")
    #expect(source.kind == .transcode(bitrate: 128))
    #expect(source.mimeType == "audio/aac")
    #expect(engine.calls.last == .load(source.url, autoplay: true))
    #expect(player.current == first)
    #expect(player.duration == 100)
    #expect(player.stateLine == PlayerModel.idleStateLine)
    #expect(player.canGoNext)
    #expect(!player.canGoPrevious)

    let info = try #require(nowPlaying.last)
    #expect(info.title == "Song 1")
    #expect(info.artist == "Era 1")
    #expect(info.album == "Era 1")
    #expect(info.artworkURL?.absoluteString == "http://test.local/eras/1/cover?v=v1")
    #expect(info.duration == 100)
  }

  @Test func coversWithoutAVersionUseTheColourKey() {
    let player = makePlayer()
    let bare = Track(id: 7, title: "x", eraID: 3, eraName: "E", colorHex: "#336699")
    #expect(player.coverURL(for: bare)?.absoluteString == "http://test.local/eras/3/cover?v=336699")
  }

  @Test func originalQualityStreamsTheStoredFileAndTrustsItsDuration() throws {
    let player = makePlayer(settings: makeSettings([AppSettings.qualityKey: ""]))
    player.play(track(1, duration: 100), queue: [])
    #expect(engine.lastSource?.url.absoluteString == "http://test.local/songs/1/stream")
    engine.emit(.ready(duration: 104.5))
    #expect(player.duration == 104.5)
  }

  @Test func transcodesKeepTheCatalogDuration() {
    let player = makePlayer()
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    #expect(player.duration == 100)
  }

  @Test func seekingATranscodeRestartsItAtTheOffset() async throws {
    let player = makePlayer(sleeper: immediateSleeper)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    engine.emit(.playing)
    player.seek(to: 42.25)
    #expect(engine.loaded.count == 1, "the restart waits for the seeks to pause")
    await player.seekTask?.value

    let source = try #require(engine.lastSource)
    #expect(source.url.absoluteString == "http://test.local/songs/1/stream?quality=128&format=aac&start=42.25")
    #expect(source.offset == 42.25)
    #expect(engine.calls.last == .load(source.url, autoplay: true))
    #expect(player.elapsed == 42.25)
    engine.emit(.time(5))
    #expect(player.elapsed == 47.25)
    #expect(nowPlaying.last?.elapsed == 42.25)
  }

  @Test func seekingWhilePausedKeepsItPaused() async {
    let player = makePlayer(sleeper: immediateSleeper)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    engine.emit(.playing)
    player.pause()
    player.seek(to: 10)
    await player.seekTask?.value
    #expect(engine.calls.last == .load(engine.lastSource!.url, autoplay: false))
  }

  @Test func seekingBeforePlaybackStartsKeepsPlaying() async {
    let player = makePlayer(sleeper: immediateSleeper)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    // Still buffering: no `.playing` yet.
    player.seek(to: 10)
    await player.seekTask?.value
    #expect(engine.calls.last == .load(engine.lastSource!.url, autoplay: true))
  }

  @Test func aRunOfSeeksRestartsTheTranscodeOnce() async {
    let player = makePlayer(sleeper: immediateSleeper)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.playing)
    player.skip(by: PlayerModel.seekStep)
    player.skip(by: PlayerModel.seekStep)
    player.skip(by: PlayerModel.seekStep)
    engine.emit(.time(1))
    #expect(player.elapsed == 15, "the old stream's clock is ignored while a restart is pending")
    await player.seekTask?.value
    #expect(engine.loaded.count == 2)
    #expect(engine.lastSource?.url.absoluteString == "http://test.local/songs/1/stream?quality=128&format=aac&start=15")
  }

  @Test func seekingATranscodeToItsEndFinishesTheSong() {
    let player = makePlayer()
    let list = [track(1, duration: 100), track(2, duration: 90)]
    player.play(list[0], queue: list)
    engine.emit(.playing)
    player.seek(to: 100)
    #expect(player.current == list[1])
    player.seek(to: 89)
    #expect(player.current == list[1], "the last song ends instead of loading an empty stream")
    #expect(player.errorMessage == nil)
    #expect(!player.isPlaying)
    #expect(player.elapsed == 90)
  }

  @Test func seekingTheOriginalSeeksNatively() {
    let player = makePlayer(settings: makeSettings([AppSettings.qualityKey: ""]))
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: 100))
    player.seek(to: 30)
    #expect(engine.calls.last == .seek(30))
    #expect(engine.loaded.count == 1)
    player.skip(by: 500)
    #expect(engine.calls.last == .seek(100))
    player.skip(by: -500)
    #expect(engine.calls.last == .seek(0))
  }

  @Test func endedAdvancesThroughTheQueueThenStops() {
    let player = makePlayer()
    let list = [track(1, duration: 10), track(2, duration: 20)]
    player.play(list[0], queue: list)
    engine.emit(.playing)
    engine.emit(.ended)
    #expect(player.current == list[1])
    #expect(engine.lastSource?.url.absoluteString.contains("/songs/2/") == true)
    #expect(player.canGoPrevious)
    #expect(!player.canGoNext)

    engine.emit(.playing)
    engine.emit(.ended)
    #expect(player.current == list[1])
    #expect(!player.isPlaying)
    #expect(player.elapsed == 20)

    // Play after the end restarts the track.
    let loads = engine.loaded.count
    player.togglePlayPause()
    #expect(engine.loaded.count == loads + 1)
  }

  @Test func nextAndPrevious() {
    let player = makePlayer()
    let list = [track(1), track(2), track(3)]
    player.play(list[1], queue: list)
    player.next()
    #expect(player.current == list[2])
    player.previous()
    player.previous()
    #expect(player.current == list[0])
    player.previous()
    #expect(player.current == list[0])
  }

  @Test func aTrackOutsideTheGivenQueueGetsItsOwnQueue() {
    let player = makePlayer()
    player.play(track(9), queue: [track(1), track(2)])
    #expect(player.queue == [track(9)])
  }

  @Test func failedTranscodeRetriesTheOriginalAtThePosition() throws {
    let player = makePlayer()
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    engine.emit(.time(40))
    engine.emit(.failed("HTTP 503"))

    #expect(player.stateLine == PlayerModel.retryingOriginalStateLine)
    #expect(player.errorMessage == nil)
    let source = try #require(engine.lastSource)
    #expect(source.kind == .original)
    #expect(engine.calls.last == .load(source.url, autoplay: true))

    engine.emit(.time(0))
    #expect(player.elapsed == 40, "position is held until the pending seek lands")
    engine.emit(.ready(duration: 100))
    #expect(engine.calls.last == .seek(40))
    #expect(player.stateLine == PlayerModel.idleStateLine)
  }

  @Test func unplayableOriginalIsConvertedToAAC() throws {
    let player = makePlayer(settings: makeSettings([AppSettings.qualityKey: ""]))
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.failed("Cannot Open"))
    #expect(player.stateLine == PlayerModel.convertingStateLine)
    let source = try #require(engine.lastSource)
    #expect(source.url.absoluteString == "http://test.local/songs/1/stream?quality=256&format=aac")
  }

  @Test func failingEveryStreamShowsAnErrorAndRetryRecovers() {
    let player = makePlayer()
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.playing)
    engine.emit(.failed("transcode"))
    engine.emit(.failed("original"))
    #expect(player.errorMessage == PlayerModel.loadFailedMessage)
    #expect(!player.isPlaying)
    #expect(player.stateLine == PlayerModel.idleStateLine)

    let loads = engine.loaded.count
    player.retry()
    #expect(player.errorMessage == nil)
    #expect(engine.loaded.count == loads + 1)
    #expect(engine.lastSource?.kind == .transcode(bitrate: 128))
  }

  @Test func qualitySwitchResumesAtThePosition() throws {
    let settings = makeSettings()
    let player = makePlayer(settings: settings)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    engine.emit(.playing)
    engine.emit(.time(50))

    player.setQuality(.original)
    #expect(settings.quality == .original)
    #expect(player.isSwitchingQuality)
    let source = try #require(engine.lastSource)
    #expect(source.kind == .original)
    #expect(engine.calls.last == .load(source.url, autoplay: true))

    engine.emit(.ready(duration: 100))
    #expect(engine.calls.last == .seek(50))
    #expect(!player.isSwitchingQuality)
    #expect(player.errorMessage == nil)
  }

  @Test func failedQualitySwitchRestoresThePreviousStream() throws {
    let settings = makeSettings()
    let player = makePlayer(settings: settings)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    engine.emit(.playing)
    engine.emit(.time(50))

    player.setQuality(.kbps320)
    #expect(engine.lastSource?.url.absoluteString.contains("quality=320") == true)
    engine.emit(.failed("HTTP 503"))

    #expect(player.errorMessage == PlayerModel.qualitySwitchFailedMessage)
    #expect(!player.isSwitchingQuality)
    let restored = try #require(engine.lastSource)
    #expect(restored.url.absoluteString == "http://test.local/songs/1/stream?quality=128&format=aac&start=50")
    // Like the web, the new choice stays selected for the next tracks.
    #expect(settings.quality == .kbps320)
  }

  @Test func switchingToTheKindAlreadyPlayingDoesNotReload() {
    let settings = makeSettings([AppSettings.qualityKey: ""])
    let player = makePlayer(settings: settings)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.failed("ogg"))  // now on the AAC fallback at 256
    let loads = engine.loaded.count
    player.setQuality(.kbps256)
    #expect(engine.loaded.count == loads)
    #expect(settings.quality == .kbps256)
  }

  @Test func qualityChangeWithoutATrackOnlyPersists() {
    let settings = makeSettings()
    let player = makePlayer(settings: settings)
    player.setQuality(.kbps64)
    #expect(settings.quality == .kbps64)
    #expect(engine.loaded.isEmpty)
  }

  @Test func bufferingHintAppearsAfterTheDelay() async {
    let player = makePlayer(sleeper: immediateSleeper)
    player.play(track(1), queue: [])
    engine.emit(.waiting)
    #expect(player.stateLine == PlayerModel.idleStateLine)
    await player.bufferingTask?.value
    #expect(player.stateLine == PlayerModel.bufferingStateLine)
    engine.emit(.playing)
    #expect(player.stateLine == PlayerModel.idleStateLine)
  }

  @Test func shortStallsNeverShowTheHint() async {
    let player = makePlayer(sleeper: foreverSleeper)
    player.play(track(1), queue: [])
    engine.emit(.waiting)
    let hint = player.bufferingTask
    engine.emit(.playing)
    await hint?.value
    #expect(player.stateLine == PlayerModel.idleStateLine)
    #expect(player.bufferingTask == nil)
  }

  @Test func aLiveStreamStallingAtItsEndCountsAsEnded() async {
    let player = makePlayer(sleeper: immediateSleeper)
    let list = [track(1, duration: 100), track(2, duration: 50)]
    player.play(list[0], queue: list)
    engine.emit(.playing)
    engine.emit(.time(99))
    engine.emit(.waiting)
    await player.endWatchdog?.value
    #expect(player.current == list[1])
  }

  @Test func stallsMidStreamAreJustBuffering() async {
    let player = makePlayer(sleeper: immediateSleeper)
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.playing)
    engine.emit(.time(40))
    engine.emit(.waiting)
    #expect(player.endWatchdog == nil)
    await player.bufferingTask?.value
    #expect(player.stateLine == PlayerModel.bufferingStateLine)
    #expect(player.current?.id == 1)
  }

  @Test func resumingBeforeTheGraceEndsKeepsTheTrack() async {
    let player = makePlayer(sleeper: foreverSleeper)
    player.play(track(1, duration: 100), queue: [track(1, duration: 100), track(2)])
    engine.emit(.time(99.5))
    engine.emit(.waiting)
    let watchdog = player.endWatchdog
    #expect(watchdog != nil)
    engine.emit(.playing)
    await watchdog?.value
    #expect(player.current?.id == 1)
  }

  @Test func anInvalidatedEngineReloadsTheSameStreamInPlace() throws {
    let player = makePlayer()
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: nil))
    engine.emit(.playing)
    engine.emit(.time(30))
    engine.emit(.invalidated)
    let source = try #require(engine.lastSource)
    #expect(source.url.absoluteString == "http://test.local/songs/1/stream?quality=128&format=aac&start=30")
    #expect(engine.calls.last == .load(source.url, autoplay: true))
    #expect(player.errorMessage == nil)
    #expect(player.stateLine == PlayerModel.idleStateLine)
  }

  @Test func anInvalidatedOriginalResumesPausedAtThePosition() {
    let player = makePlayer(settings: makeSettings([AppSettings.qualityKey: ""]))
    player.play(track(1, duration: 100), queue: [])
    engine.emit(.ready(duration: 100))
    player.pause()
    player.seek(to: 42)
    engine.emit(.invalidated)
    #expect(engine.lastSource?.kind == .original)
    #expect(engine.calls.last == .load(engine.lastSource!.url, autoplay: false))
    engine.emit(.ready(duration: 100))
    #expect(engine.calls.last == .seek(42))
  }

  @Test func unknownDurationIsProbed() async throws {
    http.on("/songs/1/duration", respond: .json(#"{"duration":99.5}"#))
    let player = makePlayer()
    player.play(track(1), queue: [])
    #expect(player.duration == 0)
    await player.durationTask?.value
    #expect(player.duration == 99.5)
    #expect(nowPlaying.last?.duration == 99.5)
  }

  @Test func probedDurationNeverOverridesTheFileDuration() async {
    http.on("/songs/1/duration", respond: .json(#"{"duration":99.5}"#))
    let player = makePlayer(settings: makeSettings([AppSettings.qualityKey: ""]))
    player.play(track(1), queue: [])
    engine.emit(.ready(duration: 120))
    await player.durationTask?.value
    #expect(player.duration == 120)
  }

  @Test func volumeIsAppliedAndPersisted() {
    let settings = makeSettings([AppSettings.volumeKey: "0.25"])
    let player = makePlayer(settings: settings)
    #expect(engine.volume == 0.25)
    player.volume = 0.8
    #expect(engine.volume == 0.8)
    #expect(settings.volume == 0.8)
  }

  @Test func queueGrowsOnlyForTheSameList() {
    let player = makePlayer()
    player.play(track(1), queue: [track(1)], queueID: "era:1")
    player.extendQueue([track(1), track(2)], queueID: "era:1")
    #expect(player.canGoNext)
    player.extendQueue([track(1), track(2), track(3)], queueID: "era:2")
    #expect(player.queue.count == 2)
    player.extendQueue([track(3)], queueID: "era:1")
    #expect(player.queue.count == 2)
  }

  @Test func playingClearsErrorsAndPausingUpdatesState() {
    let player = makePlayer()
    player.play(track(1), queue: [])
    engine.emit(.playing)
    #expect(player.isPlaying)
    #expect(nowPlaying.last?.isPlaying == true)
    player.togglePlayPause()
    #expect(engine.calls.last == .pause)
    #expect(!player.isPlaying)
    player.togglePlayPause()
    #expect(engine.calls.last == .play)
  }

  @Test func stopClearsTheLockScreen() {
    let player = makePlayer()
    player.play(track(1), queue: [])
    player.stop()
    #expect(player.current == nil)
    #expect(engine.calls.last == .stop)
    #expect(nowPlaying.updates.last! == nil)
    engine.emit(.playing)
    #expect(!player.isPlaying)
  }
}
