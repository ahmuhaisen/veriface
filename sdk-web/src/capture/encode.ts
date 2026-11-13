import { FaceCaptureError } from '../errors';

export const DEFAULT_JPEG_QUALITY = 0.92;

/**
 * Encodes a frame as JPEG at its full resolution, exactly as the camera delivered it: never
 * mirrored, never resized. Canvas encoders write no EXIF or location metadata.
 */
export async function encodeJpeg(bitmap: ImageBitmap, quality: number = DEFAULT_JPEG_QUALITY): Promise<Blob> {
  let blob: Blob | null;
  if (typeof OffscreenCanvas !== 'undefined') {
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const context = canvas.getContext('2d');
    if (context === null) throw new FaceCaptureError('unsupported', '2D canvas context is unavailable');
    context.drawImage(bitmap, 0, 0);
    blob = await canvas.convertToBlob({ type: 'image/jpeg', quality });
  } else {
    const canvas = document.createElement('canvas');
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext('2d');
    if (context === null) throw new FaceCaptureError('unsupported', '2D canvas context is unavailable');
    context.drawImage(bitmap, 0, 0);
    blob = await new Promise<Blob | null>((resolve) => {
      canvas.toBlob(resolve, 'image/jpeg', quality);
    });
  }
  if (blob?.type !== 'image/jpeg') throw new FaceCaptureError('unsupported', 'JPEG encoding is unavailable');
  return blob;
}
