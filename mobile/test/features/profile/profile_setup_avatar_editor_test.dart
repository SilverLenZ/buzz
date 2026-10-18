import 'package:buzz/features/profile/profile_setup_avatar_editor.dart';
import 'package:buzz/features/profile/profile_avatar_draft.dart';
import 'package:buzz/shared/emoji/emoji_avatar.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

void main() {
  testWidgets('avatar draft editor lays out within a full height sheet', (
    tester,
  ) async {
    Rect? previewRect;
    await tester.pumpWidget(
      ProviderScope(
        child: MaterialApp(
          home: ProfileSetupAvatarEditor(
            onPreviewReady: (rect) async {
              previewRect = rect;
            },
            initialDraft: ProfileUrlAvatarDraft(
              emojiAvatarDataUrl('🐝', 0xFFFFE75C),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    expect(find.text('Done'), findsOneWidget);
    expect(previewRect, isNotNull);
    expect(previewRect!.width, greaterThan(0));
    expect(previewRect!.height, greaterThan(0));
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
