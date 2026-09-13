import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/screens/attachment_kind.dart';

void main() {
  group('attachmentKindFor', () {
    test('image/* dispatches to image', () {
      expect(attachmentKindFor('image/png'), AttachmentKind.image);
    });

    test('audio/* dispatches to audio', () {
      expect(attachmentKindFor('audio/mpeg'), AttachmentKind.audio);
    });

    test('video/* dispatches to video', () {
      expect(attachmentKindFor('video/mp4'), AttachmentKind.video);
    });

    test('any other mime type dispatches to unsupported', () {
      expect(attachmentKindFor('application/pdf'), AttachmentKind.unsupported);
    });

    test('empty string dispatches to unsupported', () {
      expect(attachmentKindFor(''), AttachmentKind.unsupported);
    });
  });
}
