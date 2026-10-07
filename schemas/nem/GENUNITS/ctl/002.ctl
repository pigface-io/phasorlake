-- ************************************************************
-- Title:	PUBLIC_DVD_GENUNITS_202206.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GENUNITS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_GENUNITS_202206010000.CSV
APPEND INTO TABLE GENUNITS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
GENSETID CHAR(20),
STATIONID CHAR(10),
SETLOSSFACTOR FLOAT EXTERNAL,
CDINDICATOR CHAR(10),
AGCFLAG CHAR(2),
SPINNINGFLAG CHAR(2),
VOLTLEVEL FLOAT EXTERNAL,
REGISTEREDCAPACITY FLOAT EXTERNAL,
DISPATCHTYPE CHAR(20),
STARTTYPE CHAR(20),
MKTGENERATORIND CHAR(10),
NORMALSTATUS CHAR(10),
MAXCAPACITY FLOAT EXTERNAL,
GENSETTYPE CHAR(15),
GENSETNAME CHAR(40),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
CO2E_EMISSIONS_FACTOR FLOAT EXTERNAL,
CO2E_ENERGY_SOURCE CHAR(100),
CO2E_DATA_SOURCE CHAR(20))
-- End of Script
