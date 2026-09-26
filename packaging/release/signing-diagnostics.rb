# frozen_string_literal: true

require "native_packages"

# Keep native-packages' signing checks intact; expose public certificate
# diagnostics when its exact identity check would otherwise fail silently.
module SigningDiagnostics
  def execute(*arguments)
    output = super
    if arguments.first(5) == ["security", "find-identity", "-v", "-p", "codesigning"]
      expected = ENV.fetch("APPLE_SIGNING_IDENTITY")
      unless output.include?(%Q{"#{expected}"})
        warn "Signing diagnostics: valid identities in the imported keychain:"
        warn output
        warn "Signing identity has surrounding whitespace: #{expected != expected.strip}"
        keychain = arguments.last
        begin
          warn "Signing diagnostics: all identities, including trust failures:"
          warn super("security", "find-identity", "-p", "codesigning", keychain)
          pem = super("security", "find-certificate", "-a", "-p", keychain)
          pem.scan(/-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----/m).each do |block|
            cert = OpenSSL::X509::Certificate.new(block)
            warn "Certificate subject: #{cert.subject}"
            warn "Certificate issuer: #{cert.issuer}"
            warn "Certificate SHA-1: #{OpenSSL::Digest::SHA1.hexdigest(cert.to_der).upcase}"
            warn "Certificate validity: #{cert.not_before} to #{cert.not_after}"
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
