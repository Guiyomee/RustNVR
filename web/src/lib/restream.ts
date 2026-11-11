/** Host part of the RTSP republication addresses given to other programs. */
export function restreamHost(localOnly: boolean): string {
  return localOnly ? '127.0.0.1' : window.location.hostname
}
