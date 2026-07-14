// KG: TASK_333_C_Share, SPAN_333_C_Share
// Web Share API + deep links for room invitations

/**
 * Share a room invite via native share sheet
 * KG: CONTRACT_333_C_Share — navigator.share + room deep link
 */
export async function shareRoom(roomId: string, displayName?: string): Promise<boolean> {
  const url = `${window.location.origin}/333/room?id=${roomId}`;
  const title = '333 P2P Room';
  const text = displayName
    ? `${displayName} invited you to a P2P room`
    : 'Join my P2P room on 333 Platform';

  // Try native share API first (mobile)
  if (navigator.share) {
    try {
      await navigator.share({ title, text, url });
      return true;
    } catch {
      // User cancelled or share failed — fall through to clipboard
    }
  }

  // Fallback: copy to clipboard
  try {
    await navigator.clipboard.writeText(url);
    return true;
  } catch {
    return false;
  }
}

/**
 * Generate a room invite URL
 */
export function getRoomUrl(roomId: string): string {
  return `${window.location.origin}/333/room?id=${roomId}`;
}

/**
 * Parse room ID from URL (for deep link handling)
 */
export function parseRoomFromUrl(url?: string): string | null {
  const u = new URL(url || window.location.href);
  return u.searchParams.get('id');
}

/**
 * Check if Web Share API is supported
 */
export function canShare(): boolean {
  return !!navigator.share;
}
