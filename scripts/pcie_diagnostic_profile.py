def validate_pcie_diagnostic(manifest):
    if (manifest.get('format') != 7
            or manifest.get('hardware_tested') is not False
            or manifest.get('bus_master_enabled') is not False
            or manifest.get('routing_enabled') is not False
            or 'negative_test' in manifest):
        raise ValueError('Invalid PCIe discovery profile')
