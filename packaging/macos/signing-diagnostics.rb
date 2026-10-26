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

# Apple's notary service sometimes fails a submission outright (notarytool
# exits with no message), and stapling right after an accepted submission can
# fail the same way while the ticket is not yet reachable (v0.4.5's Intel DMG,
# four times; one log shows "The network connection was lost" from Apple's
# ticket service).
# Run those commands again, a bounded number of times; a result notarytool
# reports, accepted or not, is left to native-packages.
module NotaryRetry
  ATTEMPTS = 3
  DELAY = 60
  RETRIED = [%w[xcrun notarytool submit], %w[xcrun stapler staple], %w[xcrun stapler validate]].freeze

  def execute(*arguments)
    return super unless RETRIED.include?(arguments.first(3))

    attempt = 1
    begin
      super
    rescue NativePackages::Error => error
      raise if attempt >= ATTEMPTS
      # native-packages has already redacted the message.
      warn "#{arguments[1..2].join(' ')} failed (attempt #{attempt} of #{ATTEMPTS}): #{error.message.strip}"
      warn "Running it again in #{DELAY} s."
      sleep DELAY
      attempt += 1
      retry
    end
  end
end

NativePackages::MacosSigning.prepend(SigningDiagnostics)
NativePackages::MacosSigning.prepend(NotaryRetry)
# Run the executable of the same gem version the library above came from.
load Gem.bin_path("native-packages", "native-packages", NativePackages::VERSION)
