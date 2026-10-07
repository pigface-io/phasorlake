-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#NETWORK_EQUIPMENTDETAIL#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS NETWORK_EQUIPMENTDETAIL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#NETWORK_EQUIPMENTDETAIL#FILE01#202408010000.CSV
APPEND INTO TABLE NETWORK_EQUIPMENTDETAIL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SUBSTATIONID CHAR(30),
EQUIPMENTTYPE CHAR(10),
EQUIPMENTID CHAR(30),
VALIDFROM TIMESTAMP(3),
VALIDTO TIMESTAMP(3),
VOLTAGE CHAR(20),
DESCRIPTION CHAR(100),
LASTCHANGED TIMESTAMP(3),
ELEMENTID FLOAT EXTERNAL)
-- End of Script
