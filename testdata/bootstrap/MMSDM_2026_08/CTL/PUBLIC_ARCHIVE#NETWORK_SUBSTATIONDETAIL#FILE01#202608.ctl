-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#NETWORK_SUBSTATIONDETAIL#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS NETWORK_SUBSTATIONDETAIL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#NETWORK_SUBSTATIONDETAIL#FILE01#202608010000.CSV
APPEND INTO TABLE NETWORK_SUBSTATIONDETAIL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SUBSTATIONID CHAR(30),
VALIDFROM TIMESTAMP(3),
VALIDTO TIMESTAMP(3),
DESCRIPTION CHAR(100),
REGIONID CHAR(10),
OWNERID CHAR(30),
LASTCHANGED TIMESTAMP(3))
-- End of Script
