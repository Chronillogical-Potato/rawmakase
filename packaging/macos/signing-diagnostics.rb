# frozen_string_literal: true

require "native_packages"

# Keep native-packages' signing checks intact; expose public certificate
# diagnostics when its exact identity check would otherwise fail silently.
# Logs are public: never print raw command output or certificate identifiers.
module SigningDiagnostics
  def execute(*arguments)
    output = super
    if arguments.first(5) == ["security", "find-identity", "-v", "-p", "codesigning"]
      expected = ENV.fetch("APPLE_SIGNING_IDENTITY")
      unless output.include?(%Q{"#{expected}"})
        warn "Signing diagnostics: expected valid identity not found."
        warn "Any valid signing identity present: #{output.match?(/^\s*\d+\) /)}"
        warn "Signing identity has surrounding whitespace: #{expected != expected.strip}"
        keychain = arguments.last
        begin
          identities = super("security", "find-identity", "-p", "codesigning", keychain)
          warn "Expected identity present before validity filtering: #{identities.include?(%Q{"#{expected}"})}"
          errors = identities.scan(/CSSMERR_[A-Z0-9_]+/).uniq
          warn "Trust error codes: #{errors.empty? ? 'none reported' : errors.join(', ')}"
          pem = super("security", "find-certificate", "-a", "-p", keychain)
          pem.scan(/-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----/m).each do |block|
            cert = OpenSSL::X509::Certificate.new(block)
            common_name = cert.subject.to_a.find { |name, _value, _type| name == "CN" }&.at(1)
            warn "Certificate matches expected identity: #{common_name == expected}"
            warn "Certificate currently within validity dates: #{cert.not_before <= Time.now && Time.now <= cert.not_after}"
          end
        rescue NativePackages::Error, OpenSSL::X509::CertificateError
          warn "Additional certificate diagnostics could not be collected."
        end
      end
    end
    output
  end
end

NativePackages::MacosSigning.prepend(SigningDiagnostics)
load Gem.bin_path("native-packages", "native-packages", "0.7.0")
