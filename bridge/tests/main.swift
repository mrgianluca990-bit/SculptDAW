import Foundation

func require(_ value: @autoclosure () -> Bool, _ message: String) throws {
    guard value() else { throw NSError(domain: "SculptBridgeE2E", code: 1, userInfo: [NSLocalizedDescriptionKey:message]) }
}
guard CommandLine.arguments.count == 3 else { fatalError("validator publication.sculptbridge origin-v1.json") }
let file = URL(fileURLWithPath:CommandLine.arguments[1])
let packet = try SculptBridgeReturn.decode(Data(contentsOf:file), at:file)
let fixture = try JSONDecoder().decode(SculptBridge.Manifest.self, from:Data(contentsOf:URL(fileURLWithPath:CommandLine.arguments[2])))
guard let original = fixture.originCue else { fatalError("Fixture needs original cue") }
let list = CueList(id:fixture.cueListID, name:"Fixture List", cues:[original])
let show = Show(name:fixture.showName, cueLists:[list])
let updated = try SculptBridgeReturn.applying(packet, to:show, destinationListID:list.id)
guard let cue = updated.cueLists[0].cues.first(where:{$0.id == packet.manifest.cueID}) else { fatalError("Missing published cue") }
try require(cue.audioElements.count == 2, "Missing stems")
try require(cue.markers.first?.name == "Renamed", "Marker rename was lost")
try require(cue.audioElements.first?.muted == true, "Mute was lost")
try require(cue.durationSeconds >= 0.2, "Audio/marker timing was truncated")
if packet.publishMode == "updateCue" {
    try require(updated.cueLists[0].cues.count == 1 && cue.id == original.id, "Original cue identity was lost")
    try require(cue.markers.first?.id == original.markers.first?.id, "Renamed marker identity was lost")
    try require(cue.audioElements.first?.outputStartChannel == 64, "Mono OUT64 was lost")
    try require(cue.audioElements.last?.outputStartChannel == 5, "Stereo OUT5/6 was lost")
    try require(cue.hasTimecode, "LTC was lost")
} else {
    try require(updated.cueLists[0].cues.count == 2 && updated.cueLists[0].cues[0] == original, "New-cue publish changed the original")
}
print("PASS real DAW publication -> Swift Show receiver: \(packet.publishMode)")
