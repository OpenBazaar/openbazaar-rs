Pod::Spec.new do |s|
  s.name           = 'ObTor'
  s.version        = '0.1.0'
  s.summary        = 'Embedded Arti (crates/mobile-tor) for the OpenBazaar app'
  s.description    = 'Dials the home node onion service with an in-process Tor client.'
  s.author         = 'OpenBazaar'
  s.homepage       = 'https://github.com/OpenBazaar/openbazaar-rs'
  s.license        = 'MIT'
  s.platforms      = { :ios => '16.4' }
  s.swift_version  = '5.9'
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = '*.swift'
  s.vendored_frameworks = 'ObMobileTor.xcframework'
  s.frameworks = 'Security', 'SystemConfiguration'
  s.libraries = 'resolv'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
