import type { Messages } from './keys';

/** Default English strings. */
export const en: Messages = {
  'selfie.NO_FACE': 'Position your face inside the oval',
  'selfie.MULTIPLE_FACES': 'Make sure only your face is visible',
  'selfie.LOW_RESOLUTION': 'The camera resolution is too low. Try another camera or device',
  'selfie.HEAD_OUT_OF_FRAME': 'Keep your whole head inside the frame',
  'selfie.TOO_CLOSE': 'Move a little further away',
  'selfie.TOO_FAR': 'Move closer to the camera',
  'selfie.YAW_OUT_OF_RANGE': 'Look straight at the camera',
  'selfie.PITCH_OUT_OF_RANGE': 'Keep your chin level and look straight ahead',
  'selfie.ROLL_OUT_OF_RANGE': 'Keep your head upright',
  'selfie.EYES_CLOSED': 'Keep your eyes open',
  'selfie.TOO_BRIGHT': 'Too much light. Avoid bright light on your face',
  'selfie.TOO_DARK': 'Too dark. Move to a brighter place',
  'selfie.BLURRY': 'The image is blurry. Hold the camera steady',
  'selfie.HOLD_STILL': 'Hold still',
  'selfie.READY': 'Perfect, capturing…',

  'document.NO_PORTRAIT': 'Place the front of your ID inside the frame',
  'document.LOW_RESOLUTION': 'The camera resolution is too low. Try another camera or device',
  'document.TOO_CLOSE': 'Move the document a little further away',
  'document.TOO_FAR': 'Move the document closer to the camera',
  'document.GLARE': 'Reduce glare: tilt the document or avoid direct light',
  'document.TOO_DARK': 'Too dark. Move to a brighter place',
  'document.TOO_BRIGHT': 'Too bright. Avoid direct light on the document',
  'document.BLURRY': 'The image is blurry. Hold the document steady',
  'document.HOLD_STILL': 'Hold still',
  'document.READY': 'Great, capturing…',

  'error.unsupported': 'This browser does not support camera capture. Please use an up-to-date browser.',
  'error.insecure-context': 'The camera can only be used on a secure (HTTPS) page.',
  'error.camera-permission-denied':
    'Camera access was denied. Allow camera access in your browser settings, then try again.',
  'error.camera-not-found': 'No suitable camera was found.',
  'error.camera-unavailable':
    'The camera could not be started. Close other apps that use the camera, then try again.',
  'error.asset-load-failed': 'The face capture files could not be loaded. Please try again later.',
  'error.asset-version-mismatch': 'Face capture is temporarily unavailable. Please try again later.',
  'error.invalid-options': 'Face capture is not configured correctly.',
  'error.not-running': 'The camera is not running.',
  'error.disposed': 'Face capture was closed.',
  'error.internal': 'Something went wrong. Please try again.',

  'ui.capture': 'Capture',
  'ui.retake': 'Retake',
  'ui.starting': 'Starting the camera…',
  'ui.captured': 'Photo captured',
  'ui.cameraPreview': 'Camera preview',
  'ui.selfieFrame': 'Selfie camera',
  'ui.documentFrame': 'Document camera',
};
