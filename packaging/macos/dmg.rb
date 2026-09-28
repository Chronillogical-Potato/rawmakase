# frozen_string_literal: true
require "fileutils"
require "tmpdir"

payload, output = ARGV
abort "usage: dmg.rb PAYLOAD OUTPUT.dmg" unless payload && output
abort "output already exists" if File.exist?(output)
Dir.mktmpdir("rawmakase-dmg-") do |directory|
  FileUtils.cp_r(payload, File.join(directory, "RAWmakase.app"), preserve: true)
  File.symlink("/Applications", File.join(directory, "Applications"))
  # Explicit HFS+ avoids host-dependent APFS image creation/verification issues.
  # hdiutil sometimes fails with "Resource busy" on CI runners; retry after
  # detaching any volume a failed attempt left mounted and removing its output.
  created = false
  3.times do |attempt|
    if system("hdiutil", "create", "-volname", "RAWmakase",
      "-srcfolder", directory, "-fs", "HFS+", "-format", "UDZO", output)
      created = true
      break
    end
    warn "hdiutil create failed (attempt #{attempt + 1} of 3)"
    system("hdiutil", "detach", "/Volumes/RAWmakase", "-force", err: File::NULL, out: File::NULL)
    FileUtils.rm_f(output)
    sleep 10 if attempt < 2
  end
  abort "DMG creation failed after 3 attempts" unless created
end
# Retry transient DiskImages errors; persistent verification failures are fatal.
verified = false
3.times do |attempt|
  if system("hdiutil", "verify", output)
    verified = true
    break
  end
  sleep 2 if attempt < 2
end
abort "DMG verification failed" unless verified
