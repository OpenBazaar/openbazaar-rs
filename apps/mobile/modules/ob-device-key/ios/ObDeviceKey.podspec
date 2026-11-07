Pod::Spec.new do |s|
  s.name           = 'ObDeviceKey'
  s.version        = '0.1.0'
  s.summary        = 'Secure Enclave device keys for the OpenBazaar app'
  s.description    = 'P-256 keys that sign home node requests.'
  s.author         = 'OpenBazaar'
  s.homepage       = 'https://github.com/OpenBazaar/openbazaar-rs'
  s.license        = 'MIT'
  s.platforms      = { :ios => '16.4' }
  s.swift_version  = '5.9'
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = '*.swift'
  s.frameworks = 'Security', 'LocalAuthentication'
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
